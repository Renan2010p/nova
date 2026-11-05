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


//! `stand.efi`: the Nova stand's UEFI boot loader. It reads `stand.conf`
//! from the volume it was started from, shows the menu, and boots the
//! chosen entry by the entry's protocol; it knows no operating system
//! itself. This file is the configuration: which protocols this build
//! has, under the names `stand.conf` gives them, as the kernel's `conf`
//! chooses its ABIs.
//!
//! If an entry cannot boot, or the program it started returns, the error
//! is shown and the menu comes back, as Limine does.

#![no_std]
#![no_main]
#![deny(unsafe_code)]

use core::fmt::Write as _;

use common::config::{Config, Entry};
use common::menu::{Canvas, Choice, Key, Menu, Rgb};
use efi::Console;

macro_rules! printf {
    ($($arg:tt)*) => {{
        let _ = write!(Console, $($arg)*);
    }};
}

/// Where `stand.conf` is looked for, in order.
const CONFIG_PATHS: &[&str] = &["/boot/stand/stand.conf", "/boot/stand.conf", "/stand.conf", "/EFI/BOOT/stand.conf"];
/// The most entries a menu shows.
const MAX_ENTRIES: usize = 32;
/// `timeout` when `stand.conf` gives none (Limine's).
const DEFAULT_TIMEOUT: u32 = 5;

/// The firmware's entry point.
#[allow(unsafe_code)]
#[unsafe(export_name = "efi_main")]
extern "efiapi" fn efi_main(image: efi::Handle, st: *mut efi::SystemTable) -> efi::Status {
    // SAFETY: the firmware's image handle and system table, given once.
    unsafe { efi::init(image, st) };
    printf!("Nova stand {}\n", env!("CARGO_PKG_VERSION"));
    let Some((pages, len)) = CONFIG_PATHS.iter().find_map(|p| efi::read_file(p).ok()) else {
        printf!("stand: config file not found (");
        for (i, p) in CONFIG_PATHS.iter().enumerate() {
            printf!("{}{}", if i > 0 { ", " } else { "" }, p);
        }
        printf!(")\n");
        return efi::Status::LOAD_ERROR;
    };
    let text = core::str::from_utf8(pages.bytes().get(..len).unwrap_or(&[])).unwrap_or("");
    run(&Config::new(text))
}

/// The menu and booting, again after every failure.
fn run(config: &Config<'_>) -> efi::Status {
    // Entries a protocol boots (Limine's directories, with none, are not).
    let mut entries = [None::<Entry<'_>>; MAX_ENTRIES];
    let mut items = [common::menu::Entry::default(); MAX_ENTRIES];
    let mut n = 0;
    for e in config.entries().filter(|e| e.get("protocol").is_some()) {
        let (Some(slot), Some(item)) = (entries.get_mut(n), items.get_mut(n)) else { break };
        *slot = Some(e);
        *item = common::menu::Entry { label: e.name, comment: e.get("comment").unwrap_or("") };
        n += 1;
    }
    if n == 0 {
        printf!("stand: no entry in the config file\n");
        return efi::Status::LOAD_ERROR;
    }
    let default = config.get("default_entry").and_then(|v| v.parse::<usize>().ok()).map_or(0, |d| d.saturating_sub(1).min(n - 1));
    // `timeout`: seconds; "no" waits for a key; 0 boots at once.
    let mut timeout = match config.get("timeout") {
        Some(v) if v.eq_ignore_ascii_case("no") => None,
        Some(v) => Some(v.parse().unwrap_or(DEFAULT_TIMEOUT)),
        None => Some(DEFAULT_TIMEOUT),
    };
    let branding = config.get("interface_branding").unwrap_or(concat!("Nova stand ", env!("CARGO_PKG_VERSION"), " (x86-64, UEFI)"));
    loop {
        let m = Menu {
            branding,
            timeout,
            firmware_setup: efi::firmware_setup_supported(),
            default,
            entries: items.get(..n).unwrap_or(&[]),
        };
        let choice = match efi::Surface::new() {
            Some(s) if timeout != Some(0) => common::menu::run(&Screen(s), &m, key, |ms| efi::stall(usize::try_from(ms).unwrap_or(0) * 1000)),
            _ => Choice::Boot(default),
        };
        Console::clear();
        let Choice::Boot(i) = choice else { efi::reboot_to_firmware() };
        let Some(entry) = entries.get(i).copied().flatten() else { continue };
        boot(&entry);
        // Back to the menu, and no countdown this time.
        printf!("\nPress a key to return to the menu.\n");
        while efi::read_key().is_none() {
            efi::stall(10_000);
        }
        timeout = None;
    }
}

/// Boots `entry` by its protocol; returns if it could not, having said why.
fn boot(entry: &Entry<'_>) {
    let protocol = entry.get("protocol").unwrap_or("");
    match protocol {
        "freebsd" => match freebsd_amd64_efi::boot(entry) {
            Ok(never) => match never {},
            Err(e) => printf!("{}: cannot boot: {:?}\n", entry.name, e),
        },
        "linux" => match linux::boot(entry) {
            Ok(never) => match never {},
            Err(e) => printf!("{}: cannot boot: {:?}\n", entry.name, e),
        },
        "efi" | "efi_chainload" => match chainload::boot(entry) {
            Ok(st) if st == efi::Status::SUCCESS => printf!("{}: returned\n", entry.name),
            Ok(st) => printf!("{}: returned {:?}\n", entry.name, st),
            Err(e) => printf!("{}: cannot start: {:?}\n", entry.name, e),
        },
        other => printf!("{}: unknown protocol \"{}\"\n", entry.name, other),
    }
}

/// The next key, as the menu understands it.
fn key() -> Option<Key> {
    efi::read_key().map(|k| match k {
        efi::Key::Up => Key::Up,
        efi::Key::Down => Key::Down,
        efi::Key::Right => Key::Right,
        efi::Key::Home => Key::Home,
        efi::Key::End => Key::End,
        efi::Key::Escape => Key::Escape,
        efi::Key::Enter => Key::Enter,
        efi::Key::Char(c) => Key::Char(c),
        efi::Key::Other => Key::Other,
    })
}

/// The firmware's framebuffer as the menu's canvas.
struct Screen(efi::Surface);

impl Canvas for Screen {
    fn size(&self) -> (u32, u32) {
        self.0.size()
    }

    fn fill(&self, x: u32, y: u32, w: u32, h: u32, c: Rgb) {
        self.0.fill(x, y, w, h, self.0.rgb(c.0, c.1, c.2));
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    printf!("stand panic: {}\n", info);
    loop {
        core::hint::spin_loop();
    }
}
