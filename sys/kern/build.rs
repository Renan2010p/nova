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

//! The kernel's version strings (FreeBSD's `sys/conf/newvers.sh`).
//!
//! Like FreeBSD built from git, the revision is `<branch>-n<commit
//! count>-<12-digit hash>`, with `-dirty` when tracked files are modified,
//! e.g. `main-n12-446bf2d3e1a2`. The compiler line is `rustc -V`.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn run(cmd: &str, args: &[&str], dir: &PathBuf) -> Option<String> {
    let mut c = Command::new(cmd);
    if cmd == "git" {
        // Building as another user than the checkout's owner (doas make)
        // must not turn the revision into "unknown".
        c.args(["-c", "safe.directory=*"]);
    }
    let out = c.args(args).current_dir(dir).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8(out.stdout).ok()?.trim().to_owned())
}

fn main() {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let vcs = match (
        run("git", &["rev-parse", "--abbrev-ref", "HEAD"], &dir),
        run("git", &["rev-list", "--count", "HEAD"], &dir),
        run("git", &["rev-parse", "--short=12", "HEAD"], &dir),
    ) {
        (Some(branch), Some(count), Some(hash)) => {
            let dirty = run(
                "git",
                &["status", "--porcelain", "--untracked-files=no"],
                &dir,
            )
            .is_some_and(|s| !s.is_empty());
            format!(
                "{branch}-n{count}-{hash}{}",
                if dirty { "-dirty" } else { "" }
            )
        }
        _ => String::from("unknown"),
    };
    let rustc = env::var("RUSTC").unwrap_or_else(|_| String::from("rustc"));
    let compiler =
        run(&rustc, &["-V"], &dir).unwrap_or_else(|| String::from("rustc (unknown version)"));

    println!("cargo:rustc-env=NOVA_VCS={vcs}");
    println!("cargo:rustc-env=NOVA_COMPILER={compiler}");
    for f in ["HEAD", "index", "logs/HEAD"] {
        println!("cargo:rerun-if-changed=../../.git/{f}");
    }
    // Editing a source file must also refresh `-dirty`. (Not `sys/` as a
    // whole: that would include `target/`, which every build changes.)
    for d in ["arch", "compat", "conf", "framework", "kern", "libkern", "mdi", "sys", "vm"] {
        println!("cargo:rerun-if-changed=../{d}");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
