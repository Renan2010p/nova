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

//! Selects the linker script for the target architecture
//! (`sys/conf/ldscript.<arch>`, as in FreeBSD).

use std::env;

fn main() {
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if os != "none" {
        // Not a kernel build (e.g. `cargo check` for the host).
        return;
    }
    let ldscript = match arch.as_str() {
        "x86_64" => "ldscript.amd64",
        other => panic!("Nova: no linker script for target architecture {other}"),
    };
    let dir = env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    println!("cargo:rustc-link-arg-bins=-T{dir}/{ldscript}");
    println!("cargo:rerun-if-changed={ldscript}");
    println!("cargo:rerun-if-changed=build.rs");
}
