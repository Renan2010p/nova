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

//! A demangler for Rust "v0" symbol names, for kernel backtraces
//! (`#3 0x... at <kern::kern_environment::StaticEnv>::getenv+0x2a`).
//!
//! It follows the v0 grammar (RFC 2603) and prints like `rustc-demangle`'s
//! alternate form: no crate disambiguators, no instantiating crate, no
//! `.llvm.*` suffix. It needs no allocator. Constructs it does not
//! support (function pointer and `dyn` types, punycode identifiers) make
//! it fail cleanly, and the caller prints the raw symbol instead.

use core::fmt::{self, Write};

/// Writes the demangled form of `sym` to `out`. Returns false, having
/// written nothing, if `sym` is not a v0 symbol this demangler handles.
pub fn demangle(sym: &str, out: &mut impl Write) -> bool {
    // Validate into a sink first, so a failure halfway writes nothing.
    if run(sym, &mut Sink).is_err() {
        return false;
    }
    run(sym, out).is_ok()
}

struct Sink;

impl Write for Sink {
    fn write_str(&mut self, _: &str) -> fmt::Result {
        Ok(())
    }
}

/// Recursion bound against hostile or corrupt input.
const MAX_DEPTH: u32 = 64;

fn run(sym: &str, out: &mut dyn Write) -> Result<(), ()> {
    let sym = sym.split('.').next().unwrap_or(sym);
    let body = sym.strip_prefix("_R").ok_or(())?;
    // An optional encoding version (a decimal number) may precede the path.
    let body = body.trim_start_matches(|c: char| c.is_ascii_digit());
    let mut p = Parser {
        s: body.as_bytes(),
        pos: 0,
        depth: 0,
        muted: false,
        out,
    };
    p.path(true)?;
    // What may follow (instantiating crate) is not printed.
    Ok(())
}

struct Parser<'s, 'o> {
    s: &'s [u8],
    pos: usize,
    depth: u32,
    /// Parse without printing (for impl paths).
    muted: bool,
    out: &'o mut dyn Write,
}

impl<'s> Parser<'s, '_> {
    fn print(&mut self, s: &str) -> Result<(), ()> {
        if self.muted {
            return Ok(());
        }
        self.out.write_str(s).map_err(|_| ())
    }

    fn print_num(&mut self, prefix: &str, n: u64, suffix: &str) -> Result<(), ()> {
        if self.muted {
            return Ok(());
        }
        write!(self.out, "{prefix}{n}{suffix}").map_err(|_| ())
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn next(&mut self) -> Result<u8, ()> {
        let b = self.peek().ok_or(())?;
        self.pos += 1;
        Ok(b)
    }

    fn eat(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// `base-62-number`: `_` is 0, otherwise digits then `_`, plus one.
    fn integer_62(&mut self) -> Result<u64, ()> {
        if self.eat(b'_') {
            return Ok(0);
        }
        let mut x: u64 = 0;
        while !self.eat(b'_') {
            let d = match self.next()? {
                c @ b'0'..=b'9' => c - b'0',
                c @ b'a'..=b'z' => 10 + (c - b'a'),
                c @ b'A'..=b'Z' => 36 + (c - b'A'),
                _ => return Err(()),
            };
            x = x
                .checked_mul(62)
                .and_then(|x| x.checked_add(u64::from(d)))
                .ok_or(())?;
        }
        x.checked_add(1).ok_or(())
    }

    /// `[tag base-62-number]`, 0 when absent.
    fn opt_integer_62(&mut self, tag: u8) -> Result<u64, ()> {
        if !self.eat(tag) {
            return Ok(0);
        }
        self.integer_62()?.checked_add(1).ok_or(())
    }

    /// `decimal-number = "0" | [1-9] {digit}`: a leading 0 stands alone.
    fn decimal(&mut self) -> Result<usize, ()> {
        let start = self.pos;
        if self.eat(b'0') {
            return Ok(0);
        }
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        let digits = core::str::from_utf8(&self.s[start..self.pos]).map_err(|_| ())?;
        digits.parse().map_err(|_| ())
    }

    /// `identifier = [disambiguator] undisambiguated-identifier`.
    fn ident(&mut self) -> Result<(u64, &'s str), ()> {
        let dis = self.opt_integer_62(b's')?;
        if self.peek() == Some(b'u') {
            return Err(()); // punycode: not supported
        }
        let len = self.decimal()?;
        self.eat(b'_');
        let input: &'s [u8] = self.s;
        let end = self
            .pos
            .checked_add(len)
            .filter(|&e| e <= input.len())
            .ok_or(())?;
        let name = core::str::from_utf8(&input[self.pos..end]).map_err(|_| ())?;
        self.pos = end;
        Ok((dis, name))
    }

    /// Runs `f` with the input repositioned at a backref target.
    fn backref<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, ()>) -> Result<T, ()> {
        let start = self.pos - 1;
        let target = usize::try_from(self.integer_62()?).map_err(|_| ())?;
        if target >= start {
            return Err(());
        }
        let saved = core::mem::replace(&mut self.pos, target);
        let r = f(self);
        self.pos = saved;
        r
    }

    fn enter(&mut self) -> Result<(), ()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            Err(())
        } else {
            Ok(())
        }
    }

    /// `path`. `value` selects the turbofish (`f::<T>`) for generic args.
    fn path(&mut self, value: bool) -> Result<(), ()> {
        self.enter()?;
        let r = self.path_inner(value);
        self.depth -= 1;
        r
    }

    fn path_inner(&mut self, value: bool) -> Result<(), ()> {
        match self.next()? {
            b'C' => {
                let (_, name) = self.ident()?;
                self.print(name)
            }
            b'N' => {
                let ns = self.next()?;
                self.path(value)?;
                let (dis, name) = self.ident()?;
                if ns.is_ascii_lowercase() {
                    if !name.is_empty() {
                        self.print("::")?;
                        self.print(name)?;
                    }
                    Ok(())
                } else {
                    let kind = match ns {
                        b'C' => "closure",
                        b'S' => "shim",
                        _ => return Err(()),
                    };
                    self.print("::{")?;
                    self.print(kind)?;
                    if !name.is_empty() {
                        self.print(":")?;
                        self.print(name)?;
                    }
                    self.print_num("#", dis, "}")
                }
            }
            b'M' => {
                self.impl_path()?;
                self.print("<")?;
                self.ty()?;
                self.print(">")
            }
            b'X' => {
                self.impl_path()?;
                self.print("<")?;
                self.ty()?;
                self.print(" as ")?;
                self.path(false)?;
                self.print(">")
            }
            b'Y' => {
                self.print("<")?;
                self.ty()?;
                self.print(" as ")?;
                self.path(false)?;
                self.print(">")
            }
            b'I' => {
                self.path(value)?;
                self.print(if value { "::<" } else { "<" })?;
                let mut first = true;
                while !self.eat(b'E') {
                    if !first {
                        self.print(", ")?;
                    }
                    first = false;
                    self.generic_arg()?;
                }
                self.print(">")
            }
            b'B' => self.backref(|p| p.path(value)),
            _ => Err(()),
        }
    }

    /// `impl-path = [disambiguator] path`; not printed.
    fn impl_path(&mut self) -> Result<(), ()> {
        self.opt_integer_62(b's')?;
        let saved = core::mem::replace(&mut self.muted, true);
        let r = self.path(false);
        self.muted = saved;
        r
    }

    fn generic_arg(&mut self) -> Result<(), ()> {
        if self.eat(b'L') {
            self.integer_62()?;
            self.print("'_")
        } else if self.eat(b'K') {
            self.konst()
        } else {
            self.ty()
        }
    }

    fn basic_type(c: u8) -> Option<&'static str> {
        Some(match c {
            b'a' => "i8",
            b'b' => "bool",
            b'c' => "char",
            b'd' => "f64",
            b'e' => "str",
            b'f' => "f32",
            b'h' => "u8",
            b'i' => "isize",
            b'j' => "usize",
            b'l' => "i32",
            b'm' => "u32",
            b'n' => "i128",
            b'o' => "u128",
            b's' => "i16",
            b't' => "u16",
            b'u' => "()",
            b'v' => "...",
            b'x' => "i64",
            b'y' => "u64",
            b'z' => "!",
            b'p' => "_",
            _ => return None,
        })
    }

    fn ty(&mut self) -> Result<(), ()> {
        self.enter()?;
        let r = self.ty_inner();
        self.depth -= 1;
        r
    }

    fn ty_inner(&mut self) -> Result<(), ()> {
        let c = self.peek().ok_or(())?;
        if let Some(name) = Self::basic_type(c) {
            self.pos += 1;
            return self.print(name);
        }
        match c {
            b'R' | b'Q' => {
                self.pos += 1;
                if self.eat(b'L') {
                    self.integer_62()?;
                }
                self.print(if c == b'R' { "&" } else { "&mut " })?;
                self.ty()
            }
            b'P' | b'O' => {
                self.pos += 1;
                self.print(if c == b'P' { "*const " } else { "*mut " })?;
                self.ty()
            }
            b'A' => {
                self.pos += 1;
                self.print("[")?;
                self.ty()?;
                self.print("; ")?;
                self.konst()?;
                self.print("]")
            }
            b'S' => {
                self.pos += 1;
                self.print("[")?;
                self.ty()?;
                self.print("]")
            }
            b'T' => {
                self.pos += 1;
                self.print("(")?;
                let mut n = 0;
                while !self.eat(b'E') {
                    if n > 0 {
                        self.print(", ")?;
                    }
                    self.ty()?;
                    n += 1;
                }
                self.print(if n == 1 { ",)" } else { ")" })
            }
            b'B' => {
                self.pos += 1;
                self.backref(|p| p.ty())
            }
            b'D' => {
                self.pos += 1;
                self.dyn_bounds()
            }
            b'F' => Err(()), // fn pointers: not supported
            _ => self.path(false),
        }
    }

    /// `dyn-bounds lifetime`: `dyn Trait<A, Assoc = T> + Send`.
    fn dyn_bounds(&mut self) -> Result<(), ()> {
        if self.eat(b'G') {
            self.integer_62()?; // binder of higher-ranked lifetimes
        }
        self.print("dyn ")?;
        let mut first = true;
        while !self.eat(b'E') {
            if !first {
                self.print(" + ")?;
            }
            first = false;
            self.dyn_trait()?;
        }
        if !self.eat(b'L') {
            return Err(());
        }
        self.integer_62()?;
        Ok(())
    }

    /// `dyn-trait = path {"p" ident type}`: associated type bindings go
    /// inside the trait's generic arguments.
    fn dyn_trait(&mut self) -> Result<(), ()> {
        self.enter()?;
        let mut opened = false;
        let mut args = 0;
        if self.eat(b'I') {
            self.path(false)?;
            self.print("<")?;
            opened = true;
            while !self.eat(b'E') {
                if args > 0 {
                    self.print(", ")?;
                }
                self.generic_arg()?;
                args += 1;
            }
        } else {
            self.path(false)?;
        }
        while self.eat(b'p') {
            self.print(match (opened, args) {
                (false, _) => "<",
                (true, 0) => "",
                (true, _) => ", ",
            })?;
            opened = true;
            let (_, name) = self.ident()?;
            self.print(name)?;
            self.print(" = ")?;
            self.ty()?;
            args += 1;
        }
        if opened {
            self.print(">")?;
        }
        self.depth -= 1;
        Ok(())
    }

    /// `const`: integers, bools, placeholders and backrefs.
    fn konst(&mut self) -> Result<(), ()> {
        match self.next()? {
            b'p' => self.print("_"),
            b'B' => self.backref(|p| p.konst()),
            b'b' => match self.const_hex()? {
                0 => self.print("false"),
                1 => self.print("true"),
                _ => Err(()),
            },
            c if b"hmytjoaslxin".contains(&c) => {
                let neg = self.eat(b'n');
                let v = self.const_hex()?;
                self.print_num(if neg { "-" } else { "" }, v, "")
            }
            _ => Err(()),
        }
    }

    fn const_hex(&mut self) -> Result<u64, ()> {
        let mut v: u64 = 0;
        loop {
            let c = self.next()?;
            if c == b'_' {
                return Ok(v);
            }
            let d = (c as char).to_digit(16).ok_or(())?;
            v = v
                .checked_mul(16)
                .and_then(|v| v.checked_add(u64::from(d)))
                .ok_or(())?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::demangle;

    fn d(s: &str) -> Option<String> {
        let mut out = String::new();
        demangle(s, &mut out).then_some(out)
    }

    #[test]
    fn plain_paths() {
        assert_eq!(
            d("_RNvCs9IM138e3Wwa_9framework6___init").unwrap(),
            "framework::__init"
        );
        assert_eq!(
            d("_RNvNtCs4rXWqnSH08G_4kern10kern_clock9hardclock").unwrap(),
            "kern::kern_clock::hardclock"
        );
    }

    #[test]
    fn inherent_impls_and_backrefs() {
        assert_eq!(
            d("_RNvMNtCs4rXWqnSH08G_4kern16kern_environmentNtB2_9StaticEnv10getenv_u32").unwrap(),
            "<kern::kern_environment::StaticEnv>::getenv_u32"
        );
    }

    #[test]
    fn generics_consts_and_instantiating_crate() {
        assert_eq!(
            d("_RNvMs_NtCsf3jlrtbNdqs_3sys7physmemINtB4_7PhysMapKj80_E3addCs8I05nsZyQOu_5amd64")
                .unwrap(),
            "<sys::physmem::PhysMap<128>>::add"
        );
        assert_eq!(
            d("_RNvMNtCshjOLOsHrHBn_7libkern5buddyINtB2_5BuddyNtNtNtCs9IM138e3Wwa_9framework2mm7vm_phys9DmapLinksKjd_E4freeBO_")
                .unwrap(),
            "<libkern::buddy::Buddy<framework::mm::vm_phys::DmapLinks, 13>>::free"
        );
        assert_eq!(
            d("_RINvNtCsbQJWu2zlgTK_4core9panicking13assert_failedyyEB4_").unwrap(),
            "core::panicking::assert_failed::<u64, u64>"
        );
    }

    #[test]
    fn closures_and_llvm_suffix() {
        assert_eq!(
            d("_RNCNvNtNtCsgAZEfjhU0at_17compiler_builtins3mem5impls13compare_bytess1_0B7_.llvm.2074388861103626067")
                .unwrap(),
            "compiler_builtins::mem::impls::compare_bytes::{closure#3}"
        );
    }

    #[test]
    fn nested_closures() {
        assert_eq!(
            d("_RINvNtCs9IM138e3Wwa_9framework4task5spawnNCNCNvNtCs8Shlh2n0nJH_4kern13subr_selftest5mutex00EBL_").unwrap(),
            "framework::task::spawn::<kern::subr_selftest::mutex::{closure#0}::{closure#0}>"
        );
    }

    #[test]
    fn dyn_traits() {
        assert_eq!(
            d(concat!(
                "_RNvXst_NtCsdFCTIYpBdc3_5alloc5boxedINtB5_3BoxDINtNtNtCsbQJWu2zlgTK_4core3ops8function6FnOnceuE",
                "p6OutputuNtNtBP_6marker4SendEL_EIBJ_uE9call_onceCs9IM138e3Wwa_9framework"
            ))
            .unwrap(),
            "<alloc::boxed::Box<dyn core::ops::function::FnOnce<(), Output = ()> + core::marker::Send> \
             as core::ops::function::FnOnce<()>>::call_once"
        );
    }

    #[test]
    fn rejects_garbage_without_output() {
        for s in [
            "",
            "_R",
            "_RNvC",
            "hammer_time",
            "_RB_",
            "_RNvCs_3abc99xyz",
            "_RFE",
        ] {
            let mut out = String::new();
            assert!(!demangle(s, &mut out), "{s}");
            assert!(out.is_empty());
        }
    }
}
