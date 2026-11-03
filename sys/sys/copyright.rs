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

//! The notice printed first on every boot (FreeBSD's `sys/copyright.h`).

/// Copyright banner, printed by `mi_startup` before anything else.
pub const COPYRIGHT: &str = "\
Copyright (c) 2026 Renan Lucas Vieira Hilário.
Nova Project, the kernel of the Renux \"Nova\" Project.
Licensed under the Mozilla Public License, v. 2.0.
";

/// Operating system name, as `uname -s` will eventually report it for
/// native Nova programs (the FreeBSD personality reports its own).
pub const OSTYPE: &str = "Nova";
