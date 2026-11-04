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

/*
 * abidump: prints the layout of FreeBSD's ABI structures and the values of
 * its constants, as the host's compiler and headers see them, so that
 * sys/compat/freebsd can check its definitions against FreeBSD itself
 * (`make abidump`). Run it on the FreeBSD release the personality targets.
 */

#include <sys/types.h>
#include <sys/filio.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/ttycom.h>
#include <fcntl.h>
#include <stddef.h>
#include <stdio.h>
#include <termios.h>

#define	SIZE(t)		printf("size  %-24s %zu\n", #t, sizeof(t))
#define	OFF(t, f)	printf("off   %-24s %zu\n", #t "." #f, offsetof(t, f))
#define	CONST(c)	printf("const %-24s %#lx\n", #c, (unsigned long)(c))

int
main(void)
{
	SIZE(struct stat);
	OFF(struct stat, st_dev);
	OFF(struct stat, st_ino);
	OFF(struct stat, st_nlink);
	OFF(struct stat, st_mode);
	OFF(struct stat, st_uid);
	OFF(struct stat, st_gid);
	OFF(struct stat, st_rdev);
	OFF(struct stat, st_atim);
	OFF(struct stat, st_mtim);
	OFF(struct stat, st_ctim);
	OFF(struct stat, st_birthtim);
	OFF(struct stat, st_size);
	OFF(struct stat, st_blocks);
	OFF(struct stat, st_blksize);
	OFF(struct stat, st_flags);
	SIZE(struct termios);
	OFF(struct termios, c_iflag);
	OFF(struct termios, c_oflag);
	OFF(struct termios, c_cflag);
	OFF(struct termios, c_lflag);
	OFF(struct termios, c_cc);
	OFF(struct termios, c_ispeed);
	OFF(struct termios, c_ospeed);
	SIZE(struct winsize);
	CONST(TIOCGETA);
	CONST(TIOCSETA);
	CONST(TIOCSETAW);
	CONST(TIOCSETAF);
	CONST(TIOCGWINSZ);
	CONST(TIOCSWINSZ);
	CONST(TIOCGPGRP);
	CONST(TIOCSPGRP);
	CONST(FIONBIO);
	CONST(FIOCLEX);
	CONST(FIONCLEX);
	CONST(S_IFCHR);
	CONST(S_IFDIR);
	CONST(S_IFREG);
	CONST(AT_FDCWD);
	CONST(AT_EMPTY_PATH);
	return (0);
}
