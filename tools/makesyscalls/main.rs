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

//! `makesyscalls`: FreeBSD's `sys/tools/makesyscalls.lua`, for the parts
//! Nova needs: the system call numbers (`sys/syscall.h`) and names
//! (`kern/syscalls.c`) of an ABI, as Rust.
//!
//! Usage: `makesyscalls <syscalls.master> <output.rs>`.
//!
//! The ABI is FreeBSD's, so its definition is FreeBSD's: the file is read
//! from a FreeBSD source tree, never written by hand, and regenerated when
//! Nova follows a new FreeBSD release (`make sysent`).

use std::fmt::Write as _;
use std::process::ExitCode;

/// One system call slot.
struct Entry {
    num: u32,
    /// Name in `syscallnames[]`.
    name: String,
    /// `SYS_*` constant, if the slot has one.
    sys: Option<String>,
}

/// The function name in a prototype such as `int write(int fd, ...);`.
fn proto_name(proto: &str) -> Option<&str> {
    let paren = proto.find('(')?;
    let head = proto[..paren].trim_end();
    let start = head.rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).map_or(0, |i| i + 1);
    Some(&head[start..])
}

fn parse(master: &str) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    let mut lines = master.lines().enumerate();
    while let Some((lineno, line)) = lines.next() {
        let err = |m: &str| format!("syscalls.master:{}: {m}", lineno + 1);
        if !line.starts_with(|c: char| c.is_ascii_digit()) {
            continue; // comments, #include, %%ABI_HEADERS%%, prototypes
        }
        let mut tok = line.split_whitespace();
        let nums = tok.next().ok_or_else(|| err("no number"))?;
        let _audit = tok.next().ok_or_else(|| err("no audit event"))?;
        let kind = tok.next().ok_or_else(|| err("no type"))?;
        let rest: Vec<&str> = tok.collect();
        let (lo, hi) = match nums.split_once('-') {
            Some((a, b)) => (a, b),
            None => (nums, nums),
        };
        let (lo, hi): (u32, u32) =
            (lo.parse().map_err(|_| err("bad number"))?, hi.parse().map_err(|_| err("bad number"))?);
        let flags: Vec<&str> = kind.split('|').collect();
        let base = flags[0];
        if base == "RESERVED" || base == "UNIMPL" {
            for num in lo..=hi {
                out.push(Entry { num, name: format!("#{num}"), sys: None });
            }
            continue;
        }
        if base == "OBSOL" {
            let name = rest.first().ok_or_else(|| err("OBSOL without a name"))?;
            out.push(Entry { num: lo, name: format!("obs_{name}"), sys: None });
            continue;
        }
        if base == "NODEF" {
            // `NUM AUE NODEF name altname argstruct rettype`: a slot for
            // loadable system calls, without a number of its own.
            let name = rest.first().ok_or_else(|| err("NODEF without a name"))?;
            out.push(Entry { num: lo, name: (*name).to_string(), sys: None });
            continue;
        }
        // A prototype in braces follows, on this line or the next ones.
        let mut proto = String::new();
        if rest.last() != Some(&"{") {
            return Err(err("expected '{'"));
        }
        loop {
            let (_, l) = lines.next().ok_or_else(|| err("unterminated entry"))?;
            if l.trim_start().starts_with('}') {
                break;
            }
            proto.push_str(l.trim());
            proto.push(' ');
        }
        let name = proto_name(&proto).ok_or_else(|| err("no function name"))?.to_string();
        let (shown, sys) = match base {
            "STD" | "NOSTD" | "SYSMUX" => (name.clone(), Some(format!("SYS_{name}"))),
            "COMPAT" => (format!("compat.{name}"), None),
            c if c.starts_with("COMPAT") => {
                let v = &c["COMPAT".len()..];
                // `sys/syscall.h` numbers compat calls from FreeBSD 7 on.
                let numbered = v.parse::<u32>().is_ok_and(|n| n >= 7);
                let sys = numbered.then(|| format!("SYS_freebsd{v}_{name}"));
                (format!("compat{v}.{name}"), sys)
            }
            other => return Err(err(&format!("unknown type {other}"))),
        };
        out.push(Entry { num: lo, name: shown, sys });
    }
    for (i, e) in out.iter().enumerate() {
        if e.num != i as u32 {
            return Err(format!("syscall {} out of order (expected {i})", e.num));
        }
    }
    Ok(out)
}

fn generate(entries: &[Entry]) -> String {
    let mut s = String::new();
    s.push_str(concat!(
        "/*-\n",
        " * SPDX-License-Identifier: MPL-2.0\n",
        " *\n",
        " * Copyright (c) 2026 Renan Lucas Vieira Hilário\n",
        " *\n",
        " * This file is part of the Nova Project, the kernel of the\n",
        " * Renux \"Nova\" Project.\n",
        " *\n",
        " * This Source Code Form is subject to the terms of the Mozilla Public\n",
        " * License, v. 2.0. If a copy of the MPL was not distributed with this\n",
        " * file, You can obtain one at https://mozilla.org/MPL/2.0/.\n",
        " *\n",
        " * DO NOT EDIT -- generated by tools/makesyscalls from FreeBSD's\n",
        " * sys/kern/syscalls.master (`make sysent`).\n",
        " */\n",
        "\n",
        "//! System call numbers (`sys/syscall.h`) and names (`kern/syscalls.c`)\n",
        "//! of the FreeBSD ABI.\n",
        "\n",
        "#![allow(dead_code, non_upper_case_globals)]\n",
        "\n",
    ));
    for e in entries {
        if let Some(sys) = &e.sys {
            let _ = writeln!(s, "pub(crate) const {sys}: u32 = {};", e.num);
        }
    }
    let _ = writeln!(s, "\n/// One past the last system call number.");
    let _ = writeln!(s, "pub(crate) const SYS_MAXSYSCALL: u32 = {};", entries.len());
    let _ = writeln!(s, "\n/// System call names, by number (`syscallnames`).");
    let _ = writeln!(s, "pub(crate) static SYSCALLNAMES: [&str; {}] = [", entries.len());
    for e in entries {
        let _ = writeln!(s, "    {:?}, // {}", e.name, e.num);
    }
    s.push_str("];\n");
    s
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let [_, master, output] = &args[..] else {
        eprintln!("usage: makesyscalls <syscalls.master> <output.rs>");
        return ExitCode::from(2);
    };
    let text = match std::fs::read_to_string(master) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("makesyscalls: {master}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let entries = match parse(&text) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("makesyscalls: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = std::fs::write(output, generate(&entries)) {
        eprintln!("makesyscalls: {output}: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
