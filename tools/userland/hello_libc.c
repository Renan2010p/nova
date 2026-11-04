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
 * The second Nova userland test: an ordinary static FreeBSD program,
 * built with the host's cc and libc. Its start-up (TLS, malloc, stdio)
 * exercises what every FreeBSD program needs from the kernel.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int
main(int argc, char **argv)
{
	char *copy;

	copy = strdup(argv[0]);
	printf("Hello from libc: argc=%d, argv[0]=%s\n", argc, copy);
	free(copy);
	return (0);
}
