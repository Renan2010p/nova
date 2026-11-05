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

//! The terminal on the framebuffer (FreeBSD's `vt_core.c`): a grid of
//! character cells, with line wrapping and scrolling.

use framework::fb::{Framebuffer, Glyph, Pixel, framebuffer};
use framework::sync::MtxSpin;
use framework::BootInfo;
use sys::font::Font;
use sys::linker::MODINFOMD_FONT;
use sys::reboot::{RB_MULTIPLE, RB_SERIAL};


/// How text is drawn (FreeBSD's `TERMINAL_KERN_ATTR` and
/// `TERMINAL_NORM_ATTR`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attr {
    /// Kernel messages: bold white on black.
    Kernel,
    /// Program output: light gray on black.
    Normal,
}

/// A partly received UTF-8 character.
#[derive(Clone, Copy)]
struct Utf8 {
    buf: [u8; 4],
    have: usize,
    need: usize,
}

struct Term {
    fb: &'static Framebuffer,
    font: Font<'static>,
    /// Size of the text area, in cells.
    cols: u32,
    rows: u32,
    /// Top left of the text area, in pixels (it is centred).
    x0: u32,
    y0: u32,
    col: u32,
    row: u32,
    utf8: Utf8,
    black: Pixel,
    white: Pixel,
    gray: Pixel,
}

static TERM: MtxSpin<Option<Term>> = MtxSpin::new(None);

/// The loader's font, if it passed one (`vt_init_font`): the metadata
/// holds its address, biased like other preload addresses.
fn loader_font(boot: &BootInfo<'_>) -> Option<Font<'static>> {
    let addr = boot.preload.kernel()?.md(MODINFOMD_FONT)?.as_u64()?;
    let pa = addr.checked_sub(boot.preload_addr_relocate)?;
    let off = usize::try_from(pa.checked_sub(boot.loaded_base)?).ok()?;
    Font::from_font_info(boot.loaded.get(off..)?)
}

/// `percent` of full intensity, as `vt_termcolors.c` computes it.
const fn level(percent: u32) -> u8 {
    let v = percent * 255 / 100;
    if v > 255 { 255 } else { v.to_le_bytes()[0] }
}

/// Starts the console on the framebuffer, if there is one and it is a
/// console (`vtterm_cnprobe` and `vt_init`): clears the screen.
///
/// As in FreeBSD, the screen is a console unless the loader chose the
/// serial one alone (`RB_SERIAL` without `RB_MULTIPLE`, i.e.
/// `console="comconsole"`; `"comconsole,efi"` gives both).
pub fn vt_init(boot: &BootInfo<'_>) {
    if boot.howto & RB_SERIAL != 0 && boot.howto & RB_MULTIPLE == 0 {
        return;
    }
    let Some(fb) = framebuffer() else { return };
    let Some(font) = loader_font(boot).or_else(sys::font::builtin) else { return };
    let cols = fb.width() / font.width;
    let rows = fb.height() / font.height;
    if cols == 0 || rows == 0 {
        return;
    }
    let black = fb.rgb(0, 0, 0);
    fb.fill(0, 0, fb.width(), fb.height(), black);
    let gray = level(75);
    let white = level(100);
    *TERM.lock() = Some(Term {
        fb,
        font,
        cols,
        rows,
        x0: (fb.width() - cols * font.width) / 2,
        y0: (fb.height() - rows * font.height) / 2,
        col: 0,
        row: 0,
        utf8: Utf8 { buf: [0; 4], have: 0, need: 0 },
        black,
        white: fb.rgb(white, white, white),
        gray: fb.rgb(gray, gray, gray),
    });
}

/// Writes one byte of UTF-8 text to the console (`vtterm_cnputc`).
/// Never blocks: if the console is busy (a panic on another CPU in the
/// middle of a line), the byte only goes to the other consoles.
pub fn vt_putc(b: u8, attr: Attr) {
    let Some(mut term) = TERM.try_lock() else { return };
    if let Some(t) = term.as_mut() {
        t.putc(b, attr);
    }
}

impl Term {
    fn putc(&mut self, b: u8, attr: Attr) {
        let u = &mut self.utf8;
        if u.need == 0 {
            let need = match b {
                0x00..=0x7f => return self.put_char(char::from(b), attr),
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf7 => 4,
                _ => return self.put_char(char::REPLACEMENT_CHARACTER, attr),
            };
            *u = Utf8 { buf: [b, 0, 0, 0], have: 1, need };
            return;
        }
        if b & 0xc0 != 0x80 {
            // A broken sequence: show it, and start over with this byte.
            u.need = 0;
            self.put_char(char::REPLACEMENT_CHARACTER, attr);
            return self.putc(b, attr);
        }
        if let Some(slot) = u.buf.get_mut(u.have) {
            *slot = b;
        }
        u.have += 1;
        if u.have < u.need {
            return;
        }
        let (buf, have) = (u.buf, u.have);
        u.need = 0;
        let c = buf
            .get(..have)
            .and_then(|s| core::str::from_utf8(s).ok())
            .and_then(|s| s.chars().next())
            .unwrap_or(char::REPLACEMENT_CHARACTER);
        self.put_char(c, attr);
    }

    fn put_char(&mut self, c: char, attr: Attr) {
        match c {
            '\n' => self.newline(),
            '\r' => self.col = 0,
            '\t' => {
                self.col = (self.col / 8 + 1) * 8;
                if self.col >= self.cols {
                    self.newline();
                }
            }
            '\u{8}' => self.col = self.col.saturating_sub(1),
            c if u32::from(c) < 0x20 || c == '\u{7f}' => {}
            c => {
                if self.col >= self.cols {
                    self.newline();
                }
                let (fg, bold) = match attr {
                    Attr::Kernel => (self.white, true),
                    Attr::Normal => (self.gray, false),
                };
                let glyph = Glyph {
                    bits: self.font.glyph(c, bold),
                    width: self.font.width,
                    height: self.font.height,
                };
                let x = self.x0 + self.col * self.font.width;
                let y = self.y0 + self.row * self.font.height;
                self.fb.draw_glyph(x, y, glyph, fg, self.black);
                self.col += 1;
            }
        }
    }

    fn newline(&mut self) {
        self.col = 0;
        if self.row + 1 < self.rows {
            self.row += 1;
        } else {
            self.fb.scroll_up(self.font.height, self.black);
        }
    }
}
