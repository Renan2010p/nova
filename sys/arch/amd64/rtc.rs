/*-
 * SPDX-License-Identifier: MPL-2.0
 *
 * Copyright (c) 2026 Renan Lucas Vieira Hilário
 *
 * This file is part of the Nova Project, the kernel of the
 * Renux "Nova" Project.
 *
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */


//! The PC's real-time clock, the MC146818 in the chipset's CMOS (FreeBSD's
//! `x86/isa/atrtc.c`): read once at boot for the wall clock. QEMU and
//! bhyve keep it in UTC, as FreeBSD expects unless `/etc/wall_cmos_clock`
//! exists.

use x86::cpufunc::{inb, outb};

const IO_RTC: u16 = 0x70;
const RTC_SEC: u8 = 0x00;
const RTC_MIN: u8 = 0x02;
const RTC_HRS: u8 = 0x04;
const RTC_DAY: u8 = 0x07;
const RTC_MONTH: u8 = 0x08;
const RTC_YEAR: u8 = 0x09;
const RTC_STATUSA: u8 = 0x0a;
const RTC_STATUSB: u8 = 0x0b;
/// The century, where ACPI's FADT usually says (`RTC_CENTURY`).
const RTC_CENTURY: u8 = 0x32;
/// `RTCSA_TUP`: an update is in progress.
const RTCSA_TUP: u8 = 0x80;
/// `RTCSB_BCD` clear: values are BCD; `RTCSB_24HR`: 24-hour clock.
const RTCSB_BIN: u8 = 0x04;
const RTCSB_24HR: u8 = 0x02;

fn rtcin(reg: u8) -> u8 {
    // SAFETY: ports 0x70/0x71 are the RTC's index and data registers on
    // every PC; reading a CMOS register has no side effect but the index.
    unsafe {
        outb(IO_RTC, reg);
        inb(IO_RTC + 1)
    }
}

fn bcd(v: u8) -> u64 {
    u64::from(v >> 4) * 10 + u64::from(v & 0xf)
}

/// Days since 1970-01-01 of a date (proleptic Gregorian, `clock_ct_to_ts`).
fn days(year: u64, month: u64, day: u64) -> Option<u64> {
    let leap = |y: u64| (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400);
    let mdays = [31u64, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if year < 1970 || !(1..=12).contains(&month) || day == 0 {
        return None;
    }
    let mut d = (1970..year).map(|y| if leap(y) { 366 } else { 365 }).sum::<u64>();
    for m in 1..month {
        d += mdays.get(usize::try_from(m - 1).ok()?)? + u64::from(m == 2 && leap(year));
    }
    Some(d + day - 1)
}

/// Reads the clock (`atrtc_gettime`): seconds since the Epoch.
pub(crate) fn rtc_gettime() -> Option<u64> {
    // Wait out an update, then read twice until both reads agree.
    let read = || {
        let mut spins = 0;
        while rtcin(RTC_STATUSA) & RTCSA_TUP != 0 && spins < 1_000_000 {
            spins += 1;
            core::hint::spin_loop();
        }
        [rtcin(RTC_SEC), rtcin(RTC_MIN), rtcin(RTC_HRS), rtcin(RTC_DAY), rtcin(RTC_MONTH), rtcin(RTC_YEAR), rtcin(RTC_CENTURY)]
    };
    let mut t = read();
    for _ in 0..5 {
        let again = read();
        if again == t {
            break;
        }
        t = again;
    }
    let sb = rtcin(RTC_STATUSB);
    let conv = |v: u8| if sb & RTCSB_BIN != 0 { u64::from(v) } else { bcd(v) };
    let [sec, min, hrs, day, month, year, century] = t;
    let pm = sb & RTCSB_24HR == 0 && hrs & 0x80 != 0;
    let mut hour = conv(hrs & 0x7f);
    if sb & RTCSB_24HR == 0 {
        hour %= 12;
        if pm {
            hour += 12;
        }
    }
    let cent = conv(century);
    let year = if (19..=21).contains(&cent) { cent * 100 + conv(year) } else { 2000 + conv(year) };
    let (sec, min) = (conv(sec), conv(min));
    if sec > 59 || min > 59 || hour > 23 {
        return None;
    }
    let d = days(year, conv(month), conv(day))?;
    Some(d * 86_400 + hour * 3600 + min * 60 + sec)
}
