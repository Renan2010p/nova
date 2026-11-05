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

//! The boot menu, laid out as Limine's (a text terminal drawn on the
//! framebuffer): the branding centred on the second line, the key help
//! under it, the entries centred on the screen with the selection in
//! reverse video, and "Booting automatically in N..." near the bottom;
//! black, light gray, cyan and green, in the PC's VGA font with 9-pixel
//! cells.
//!
//! Machine- and firmware-independent: it draws on a [`Canvas`] and reads
//! keys from a function, so any firmware layer can show it.

use sys::font::Font;

/// A colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Somewhere to draw. Every call is clipped by the implementation.
pub trait Canvas {
    /// Width and height in pixels.
    fn size(&self) -> (u32, u32);
    /// Fills a rectangle.
    fn fill(&self, x: u32, y: u32, w: u32, h: u32, c: Rgb);
}

/// A key, as the menu understands it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Previous entry.
    Up,
    /// Next entry.
    Down,
    /// Boot the selection.
    Right,
    /// First entry.
    Home,
    /// Last entry.
    End,
    /// Nothing but stopping the countdown.
    Escape,
    /// Boot the selection.
    Enter,
    /// A character: `1`-`9` boot that entry, space boots, `s` opens the
    /// firmware's setup.
    Char(char),
    /// Anything else (stops the countdown).
    Other,
}

/// A menu entry.
#[derive(Clone, Copy, Debug, Default)]
pub struct Entry<'a> {
    /// Its name.
    pub label: &'a str,
    /// Shown at the bottom while it is selected (Limine's `COMMENT`).
    pub comment: &'a str,
}

/// What was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Boot this entry (an index into [`Menu::entries`]).
    Boot(usize),
    /// Restart into the firmware's setup screen.
    FirmwareSetup,
}

/// What the menu shows.
pub struct Menu<'a> {
    /// The line at the top (Limine's `INTERFACE_BRANDING`).
    pub branding: &'a str,
    /// Seconds before booting the selected entry; `None` waits.
    pub timeout: Option<u32>,
    /// Whether `S` can open the firmware's setup.
    pub firmware_setup: bool,
    /// The entry selected at first.
    pub default: usize,
    /// The choices.
    pub entries: &'a [Entry<'a>],
}

// The terminal's colours: VGA's palette, as Limine's defaults.
const BG: Rgb = Rgb(0, 0, 0);
const FG: Rgb = Rgb(0xaa, 0xaa, 0xaa);
const CYAN: Rgb = Rgb(0x00, 0xaa, 0xaa);
const GREEN: Rgb = Rgb(0x00, 0xaa, 0x00);
const BRIGHT_GREEN: Rgb = Rgb(0x55, 0xff, 0x55);

/// A text terminal over the canvas: cells of the font's size plus one
/// column of spacing, the grid centred on the screen.
struct Term<'a, C: Canvas> {
    c: &'a C,
    font: Font<'static>,
    cw: u32,
    ch: u32,
    cols: u32,
    rows: u32,
    ox: u32,
    oy: u32,
}

impl<C: Canvas> Term<'_, C> {
    fn clear(&self) {
        let (w, h) = self.c.size();
        self.c.fill(0, 0, w, h, BG);
    }

    fn clear_row(&self, row: u32) {
        let (w, _) = self.c.size();
        self.c.fill(0, self.oy + row * self.ch, w, self.ch, BG);
    }

    /// Draws `s` from cell (`col`, `row`).
    fn put(&self, col: u32, row: u32, s: &str, fg: Rgb, bg: Rgb) -> u32 {
        let bpr = usize::try_from(self.font.width.div_ceil(8)).unwrap_or(1);
        let mut col = col;
        for chr in s.chars() {
            if col >= self.cols {
                break;
            }
            let (x, y) = (self.ox + col * self.cw, self.oy + row * self.ch);
            self.c.fill(x, y, self.cw, self.ch, bg);
            let g = self.font.glyph(chr, false);
            for r in 0..self.font.height {
                let line = usize::try_from(r).ok().and_then(|r| g.get(r * bpr..r * bpr + bpr)).unwrap_or(&[]);
                let mut c = 0;
                while c < self.font.width {
                    let set = |c: u32| usize::try_from(c / 8).ok().and_then(|i| line.get(i)).is_some_and(|b| b & (0x80 >> (c % 8)) != 0);
                    if set(c) {
                        let start = c;
                        while c < self.font.width && set(c) {
                            c += 1;
                        }
                        self.c.fill(x + start, y + r, c - start, 1, fg);
                    } else {
                        c += 1;
                    }
                }
            }
            col += 1;
        }
        col
    }

    fn len(s: &str) -> u32 {
        u32::try_from(s.chars().count()).unwrap_or(u32::MAX)
    }

    fn centre(&self, len: u32) -> u32 {
        self.cols.saturating_sub(len) / 2
    }

    /// `KEY label` pieces separated by four spaces, centred on `row`.
    fn help(&self, row: u32, items: &[(&str, &str)]) {
        let len: u32 = items.iter().map(|(k, l)| Self::len(k) + 1 + Self::len(l)).sum::<u32>()
            + 4 * u32::try_from(items.len().saturating_sub(1)).unwrap_or(0);
        let mut col = self.centre(len);
        for (i, (k, l)) in items.iter().enumerate() {
            if i > 0 {
                col += 4;
            }
            col = self.put(col, row, k, GREEN, BG);
            col = self.put(col + 1, row, l, FG, BG);
        }
    }
}

/// Lines from the top the help takes (Limine's `header_offset`).
fn header_offset(m: &Menu<'_>) -> u32 {
    (if m.branding.is_empty() { 0 } else { 2 }) + if m.firmware_setup { 2 } else { 0 }
}

fn draw<C: Canvas>(t: &Term<'_, C>, m: &Menu<'_>, sel: usize) {
    t.clear();
    if !m.branding.is_empty() {
        t.put(t.centre(Term::<C>::len(m.branding)), 1, m.branding, CYAN, BG);
    }
    let ho = header_offset(m);
    let primary = 1 + ho - if m.firmware_setup { 2 } else { 0 };
    t.help(primary, &[("ARROWS", "Select"), ("ENTER", "Boot")]);
    if m.firmware_setup {
        t.help(1 + ho, &[("S", "Firmware Setup")]);
    }
    // The tree: "   " then " name ", the selected one in reverse video.
    let max_len = m.entries.iter().map(|e| 3 + 1 + Term::<C>::len(e.label) + 1).max().unwrap_or(0);
    let prefix = if t.cols > max_len + 3 { (t.cols - max_len - 3) / 2 } else { 1 };
    let n = u32::try_from(m.entries.len()).unwrap_or(0);
    let start = ((t.rows.saturating_sub(n)) / 2).max(4 + ho);
    for (i, e) in m.entries.iter().enumerate() {
        let row = start + u32::try_from(i).unwrap_or(0);
        let (fg, bg) = if i == sel { (BG, FG) } else { (FG, BG) };
        let col = t.put(prefix + 3, row, " ", fg, bg);
        let col = t.put(col, row, e.label, fg, bg);
        t.put(col, row, " ", fg, bg);
    }
}

fn draw_comment<C: Canvas>(t: &Term<'_, C>, m: &Menu<'_>, sel: usize) {
    let row = t.rows.saturating_sub(2);
    t.clear_row(row);
    if let Some(e) = m.entries.get(sel).filter(|e| !e.comment.is_empty()) {
        t.put(t.centre(Term::<C>::len(e.comment)), row, e.comment, CYAN, BG);
    }
}

fn draw_countdown<C: Canvas>(t: &Term<'_, C>, secs: u32) {
    let row = t.rows.saturating_sub(2);
    let mut digits = [0u8; 10];
    let mut n = secs;
    let mut len = 0;
    loop {
        digits[len] = b'0' + u8::try_from(n % 10).unwrap_or(0);
        len += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    digits[..len].reverse();
    let num = core::str::from_utf8(&digits[..len]).unwrap_or("?");
    let msg_len = 28 + Term::<C>::len(num);
    t.clear_row(row);
    let col = t.put(t.centre(msg_len), row, "Booting automatically in ", GREEN, BG);
    let col = t.put(col, row, num, BRIGHT_GREEN, BG);
    t.put(col, row, "...", GREEN, BG);
}

/// Shows the menu until a choice is made, and returns it. `sleep` waits
/// that many milliseconds; `key` returns the next key pressed, if any.
pub fn run(c: &impl Canvas, m: &Menu<'_>, mut key: impl FnMut() -> Option<Key>, mut sleep: impl FnMut(u32)) -> Choice {
    let n = m.entries.len();
    let (Some(font), true) = (sys::font::vga(), n > 0) else { return Choice::Boot(m.default) };
    let (w, h) = c.size();
    let (cw, ch) = (font.width + 1, font.height);
    let (cols, rows) = (w / cw, h / ch);
    let t = Term { c, font, cw, ch, cols, rows, ox: (w % cw) / 2, oy: (h % ch) / 2 };
    let choose = Choice::Boot;
    let mut sel = m.default.min(n - 1);
    draw(&t, m, sel);

    // The countdown: whole seconds, as Limine counts them.
    let mut pending = None;
    if let Some(secs) = m.timeout {
        let mut left = secs;
        loop {
            if left == 0 {
                return choose(sel);
            }
            draw_countdown(&t, left);
            let mut waited = 0;
            while waited < 1000 && pending.is_none() {
                pending = key();
                if pending.is_none() {
                    sleep(10);
                    waited += 10;
                }
            }
            if pending.is_some() {
                break;
            }
            left -= 1;
        }
    }
    draw_comment(&t, m, sel);
    loop {
        let Some(k) = pending.take().or_else(&mut key) else {
            sleep(10);
            continue;
        };
        let before = sel;
        match k {
            Key::Char(d @ '1'..='9') => {
                let i = usize::try_from(d.to_digit(10).unwrap_or(0)).unwrap_or(0);
                if (1..=n).contains(&i) {
                    return choose(i - 1);
                }
            }
            Key::Home => sel = 0,
            Key::End => sel = n - 1,
            Key::Up => sel = (sel + n - 1) % n,
            Key::Down => sel = (sel + 1) % n,
            Key::Right | Key::Enter | Key::Char(' ') => return choose(sel),
            Key::Char('s' | 'S') if m.firmware_setup => return Choice::FirmwareSetup,
            _ => {}
        }
        if sel != before {
            draw(&t, m, sel);
            draw_comment(&t, m, sel);
        }
    }
}
