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

//! `checklayers`: enforces the layering rules of the kernel workspace.
//!
//! Usage: `checklayers <path to sys/>`. Exits 1 on any violation.
//!
//! The rules (numbered as in docs/LAYERING.md):
//!
//! * R1 dependencies only point to allowed layers (see [`may_depend`]);
//! * R2 services do not see the `mdi` contract;
//! * R3 `cfg(target_arch ...)` only in MD code, the framework, or files
//!   under an `arch/` directory of a service;
//! * R4 `unsafe` only in the trusted base: every other crate root must
//!   carry `#![forbid(unsafe_code)]` (`#![deny(unsafe_code)]` for `conf`,
//!   whose entry macro opts in, and `mdi`, which declares `unsafe trait`s);
//! * R5 `asm!` / `global_asm!` / `naked_asm!` only in MD code;
//! * R6 no `static mut` anywhere (SMP: shared state needs a lock, an
//!   atomic or `CpuLocal`);
//! * R7 intra-kernel dependencies use `workspace = true`; no external
//!   crates unless listed in [`ALLOWED_EXTERNAL`];
//! * R8 every `.rs` file starts with the MPL-2.0 header;
//! * R9 personalities (`compat/*`) are leaves: only `conf` depends on them.
//!
//! It is deliberately dependency-free and parses only the subset of TOML
//! that sys/ uses. Source checks run on code with comments and string
//! literals blanked out, so documentation may mention anything.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// External crates the kernel may use, with the reason. Empty on purpose:
/// every entry grows the trusted computing base and the supply chain.
const ALLOWED_EXTERNAL: &[(&str, &str)] = &[];

/// Required first lines of every Rust source file.
const HEADER: &str = "/*-\n * SPDX-License-Identifier: MPL-2.0\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Layer {
    /// `libkern`, `sys`, `mdi`: usable by everyone below the services.
    Base,
    /// `arch/*`: machine-dependent code.
    Md,
    /// `framework`: the framekernel's trusted core.
    Framework,
    /// Everything else: the MI kernel, 100% safe.
    Service,
    /// `conf`: the kernel binary.
    Conf,
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Layer::Base => "base",
            Layer::Md => "md",
            Layer::Framework => "framework",
            Layer::Service => "service",
            Layer::Conf => "conf",
        })
    }
}

struct Crate {
    name: String,
    /// Member path relative to sys/, e.g. `arch/amd64`.
    path: String,
    layer: Layer,
    /// Allowed to use `unsafe`.
    trusted: bool,
    deps: Vec<Dep>,
}

struct Dep {
    name: String,
    workspace: bool,
    line: usize,
}

/// Layer of a workspace member, decided by its path. New directories are
/// services by default: the strictest layer.
fn classify(path: &str) -> (Layer, bool) {
    match path {
        "libkern" => (Layer::Base, true),
        "sys" | "mdi" => (Layer::Base, false),
        "framework" => (Layer::Framework, true),
        "conf" => (Layer::Conf, false),
        p if p.starts_with("arch/") => (Layer::Md, true),
        _ => (Layer::Service, false),
    }
}

/// R1/R2/R9: may a crate in `from` depend on crate `to`?
fn may_depend(from: &Crate, to: &Crate) -> Result<(), &'static str> {
    use Layer::*;
    if to.path.starts_with("compat/") && from.layer != Conf {
        return Err("R9: personalities are leaves; only conf links them in");
    }
    let ok = match (from.layer, to.layer) {
        (Base, Base) => true,
        (Md, Base | Md) => true,
        (Framework, Base | Md) => true,
        (Service | Conf, Base) if to.name == "mdi" => {
            return Err("R2: services do not see the mdi contract; use framework");
        }
        (Service, Base | Framework | Service) => true,
        (Conf, Base | Framework | Service) => true,
        (_, Md) => return Err("R1: only framework (and other MD crates) may depend on MD crates"),
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err("R1: dependency points to a higher layer")
    }
}

struct Report {
    errors: Vec<String>,
}

impl Report {
    fn err(&mut self, file: &Path, line: usize, msg: impl fmt::Display) {
        self.errors
            .push(format!("{}:{}: {}", file.display(), line, msg));
    }
}

fn main() -> ExitCode {
    let Some(sys) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: checklayers <path to sys/>");
        return ExitCode::from(2);
    };
    match run(&sys) {
        Ok(report) if report.errors.is_empty() => ExitCode::SUCCESS,
        Ok(report) => {
            for e in &report.errors {
                eprintln!("checklayers: {e}");
            }
            eprintln!(
                "checklayers: {} violation(s); see docs/LAYERING.md",
                report.errors.len()
            );
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("checklayers: {e}");
            ExitCode::from(2)
        }
    }
}

fn run(sys: &Path) -> Result<Report, String> {
    let root_toml = sys.join("Cargo.toml");
    let root = read(&root_toml)?;
    let members = parse_members(&root);
    let ws_deps = parse_workspace_deps(&root);
    if members.is_empty() {
        return Err(format!(
            "{}: no workspace members found",
            root_toml.display()
        ));
    }

    let mut report = Report { errors: Vec::new() };
    let mut crates = Vec::new();
    for path in &members {
        let toml_path = sys.join(path).join("Cargo.toml");
        let toml = read(&toml_path)?;
        let (layer, trusted) = classify(path);
        crates.push(Crate {
            name: parse_package_name(&toml)
                .ok_or_else(|| format!("{}: no package name", toml_path.display()))?,
            path: path.clone(),
            layer,
            trusted,
            deps: parse_deps(&toml),
        });
    }
    let by_name: BTreeMap<&str, &Crate> = crates.iter().map(|c| (c.name.as_str(), c)).collect();

    for c in &crates {
        let toml_path = sys.join(&c.path).join("Cargo.toml");
        for d in &c.deps {
            match by_name.get(d.name.as_str()) {
                Some(to) => {
                    if !d.workspace {
                        report.err(
                            &toml_path,
                            d.line,
                            format!("R7: `{}` must be declared with `workspace = true`", d.name),
                        );
                    }
                    if let Err(why) = may_depend(c, to) {
                        report.err(
                            &toml_path,
                            d.line,
                            format!(
                                "{why} ({} `{}` -> {} `{}`)",
                                c.layer, c.name, to.layer, to.name
                            ),
                        );
                    }
                }
                None if ws_deps.contains_key(d.name.as_str()) => {
                    report.err(
                        &toml_path,
                        d.line,
                        format!(
                            "R7: `{}` is declared in the workspace but is not a member",
                            d.name
                        ),
                    );
                }
                None if ALLOWED_EXTERNAL.iter().any(|(n, _)| *n == d.name) => {}
                None => report.err(
                    &toml_path,
                    d.line,
                    format!("R7: external crate `{}` is not in ALLOWED_EXTERNAL", d.name),
                ),
            }
        }
        check_sources(sys, c, &mut report)?;
    }
    if report.errors.is_empty() {
        println!(
            "checklayers: {} crates, all layering rules hold",
            crates.len()
        );
    }
    Ok(report)
}

fn check_sources(sys: &Path, c: &Crate, report: &mut Report) -> Result<(), String> {
    let dir = sys.join(&c.path);
    let mut files = Vec::new();
    collect_rs(&dir, &mut files)?;
    files.sort();

    // R4: crate root attributes. `conf` (the entry macro) and `mdi`
    // (`unsafe trait` declarations) may only opt in item by item.
    let (root_name, attr) = match (c.layer, c.name.as_str()) {
        (Layer::Conf, _) => ("main.rs", "#![deny(unsafe_code)]"),
        (_, "mdi") => ("lib.rs", "#![deny(unsafe_code)]"),
        _ => ("lib.rs", "#![forbid(unsafe_code)]"),
    };
    if !c.trusted {
        let root = dir.join(root_name);
        if !read(&root)?.contains(attr) {
            report.err(
                &root,
                1,
                format!(
                    "R4: `{}` is not in the trusted base and must declare {attr}",
                    c.name
                ),
            );
        }
    }

    for file in &files {
        let raw = read(file)?;
        if !raw.starts_with(HEADER) {
            report.err(file, 1, "R8: missing MPL-2.0 header (see docs/STYLE.md)");
        }
        let rel = file
            .strip_prefix(&dir)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        let in_arch_dir = rel.starts_with("arch/") || rel.contains("/arch/");
        let code = strip_comments_and_strings(&raw);
        for (i, line) in code.lines().enumerate() {
            let n = i + 1;
            if line.contains("target_arch")
                && !matches!(c.layer, Layer::Md | Layer::Framework)
                && !in_arch_dir
            {
                report.err(
                    file,
                    n,
                    "R3: `target_arch` outside MD code; move it under an `arch/` directory",
                );
            }
            if line.contains("asm!") && c.layer != Layer::Md {
                report.err(file, n, "R5: assembly is only allowed in sys/arch");
            }
            if line
                .split_whitespace()
                .collect::<Vec<_>>()
                .windows(2)
                .any(|w| w == ["static", "mut"])
            {
                report.err(
                    file,
                    n,
                    "R6: `static mut` is forbidden; use a lock, an atomic or CpuLocal",
                );
            }
        }
    }
    Ok(())
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n != "target") {
                collect_rs(&path, out)?;
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Section headers and `key = value` lines, with 1-based line numbers.
fn toml_lines(toml: &str) -> impl Iterator<Item = (usize, Option<&str>, &str)> {
    let mut section = None;
    toml.lines().enumerate().filter_map(move |(i, line)| {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            section = Some(line);
            None
        } else if line.is_empty() {
            None
        } else {
            Some((i + 1, section, line))
        }
    })
}

fn parse_members(toml: &str) -> Vec<String> {
    let Some(start) = toml.find("members") else {
        return Vec::new();
    };
    let rest = &toml[start..];
    let Some(end) = rest.find(']') else {
        return Vec::new();
    };
    rest[..end]
        .lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .flat_map(|l| l.split('"').skip(1).step_by(2))
        .map(str::to_owned)
        .collect()
}

fn parse_workspace_deps(toml: &str) -> BTreeMap<String, String> {
    toml_lines(toml)
        .filter(|(_, s, _)| *s == Some("[workspace.dependencies]"))
        .filter_map(|(_, _, l)| {
            let (name, rest) = l.split_once('=')?;
            let path = rest.split("path").nth(1)?.split('"').nth(1)?;
            Some((name.trim().to_owned(), path.to_owned()))
        })
        .collect()
}

fn parse_package_name(toml: &str) -> Option<String> {
    toml_lines(toml)
        .filter(|(_, s, _)| *s == Some("[package]"))
        .find_map(|(_, _, l)| {
            let (k, v) = l.split_once('=')?;
            (k.trim() == "name").then(|| v.trim().trim_matches('"').to_owned())
        })
}

/// `[dependencies]` and `[target.'...'.dependencies]`; dev- and
/// build-dependencies run on the host and are not part of the kernel.
fn parse_deps(toml: &str) -> Vec<Dep> {
    toml_lines(toml)
        .filter(|(_, s, _)| {
            s.is_some_and(|s| {
                s == "[dependencies]"
                    || (s.starts_with("[target.") && s.ends_with(".dependencies]"))
            })
        })
        .filter_map(|(line, _, l)| {
            let (key, value) = l.split_once('=')?;
            let key = key.trim();
            let (name, workspace) = match key.strip_suffix(".workspace") {
                Some(name) => (name, value.trim() == "true"),
                None => (key, value.replace(' ', "").contains("workspace=true")),
            };
            Some(Dep {
                name: name.to_owned(),
                workspace,
                line,
            })
        })
        .collect()
}

/// Replaces comments and the contents of string literals with spaces,
/// keeping newlines so line numbers stay right. Handles nested block
/// comments, escapes and raw strings; char literals and lifetimes are left
/// alone (they cannot contain the patterns we look for).
fn strip_comments_and_strings(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let blank = |c: u8| if c == b'\n' { b'\n' } else { b' ' };
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                out.push(b' ');
                i += 1;
            }
        } else if b[i..].starts_with(b"/*") {
            let mut depth = 0;
            while i < b.len() {
                if b[i..].starts_with(b"/*") {
                    depth += 1;
                    out.extend_from_slice(b"  ");
                    i += 2;
                } else if b[i..].starts_with(b"*/") {
                    depth -= 1;
                    out.extend_from_slice(b"  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(b[i]));
                    i += 1;
                }
            }
        } else if b[i] == b'r'
            && matches!(b.get(i + 1), Some(b'"' | b'#'))
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric())
        {
            let hashes = b[i + 1..].iter().take_while(|&&c| c == b'#').count();
            if b.get(i + 1 + hashes) != Some(&b'"') {
                out.push(b[i]);
                i += 1;
                continue;
            }
            let close: Vec<u8> = std::iter::once(b'"')
                .chain(std::iter::repeat_n(b'#', hashes))
                .collect();
            out.extend(std::iter::repeat_n(b' ', 2 + hashes));
            i += 2 + hashes;
            while i < b.len() && !b[i..].starts_with(&close) {
                out.push(blank(b[i]));
                i += 1;
            }
            out.extend(std::iter::repeat_n(b' ', close.len().min(b.len() - i)));
            i += close.len();
        } else if b[i] == b'"' {
            out.push(b' ');
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' && i + 1 < b.len() {
                    out.push(b' ');
                    i += 1;
                }
                out.push(blank(b[i]));
                i += 1;
            }
            out.push(b' ');
            i += 1;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_comments_and_strings() {
        let s = "a // target_arch\nb /* x /* asm! */ y */ c \"static mut\" r#\"asm!\"# d\n";
        let out = strip_comments_and_strings(s);
        assert_eq!(out.lines().count(), 2);
        assert!(
            !out.contains("target_arch") && !out.contains("asm!") && !out.contains("static mut")
        );
        assert!(out.contains('a') && out.contains('c') && out.contains('d'));
    }

    #[test]
    fn layering_table() {
        let mk = |name: &str, path: &str| {
            let (layer, trusted) = classify(path);
            Crate {
                name: name.into(),
                path: path.into(),
                layer,
                trusted,
                deps: Vec::new(),
            }
        };
        let kern = mk("kern", "kern");
        let amd64 = mk("amd64", "arch/amd64");
        let fw = mk("framework", "framework");
        let mdi = mk("mdi", "mdi");
        let compat = mk("compat_freebsd", "compat/freebsd");
        let conf = mk("kernel", "conf");
        assert!(may_depend(&kern, &fw).is_ok());
        assert!(may_depend(&kern, &amd64).is_err());
        assert!(may_depend(&kern, &mdi).is_err());
        assert!(may_depend(&fw, &amd64).is_ok());
        assert!(may_depend(&amd64, &fw).is_err());
        assert!(may_depend(&amd64, &kern).is_err());
        assert!(may_depend(&kern, &compat).is_err());
        assert!(may_depend(&conf, &compat).is_ok());
    }
}
