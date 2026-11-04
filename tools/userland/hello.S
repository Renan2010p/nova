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
 * The first Nova userland test: a static FreeBSD/amd64 program with no
 * libc. It checks the initial stack (argc and argv[0], as FreeBSD's
 * crt1 reads them through %rdi), write(2) to the console, and _exit(2).
 * Booted as init, its exit ends in FreeBSD's "init died" panic.
 */

	/* FreeBSD ABI tag note: what makes the kernel pick the FreeBSD ABI. */
	.section .note.tag, "a", @note
	.p2align 2
	.long	8		/* namesz */
	.long	4		/* descsz */
	.long	1		/* NT_FREEBSD_ABI_TAG */
	.asciz	"FreeBSD"
	.long	1501000		/* __FreeBSD_version */

	.section .rodata
hello:	.ascii	"Hello from Nova userland: argc="
hello_len = . - hello
of:	.ascii	", argv[0]="
of_len = . - of
nl:	.ascii	"\n"

	.text
	.globl	_start
_start:
	movq	%rdi, %rbx		/* the stack base: argc, argv[] */

	leaq	hello(%rip), %rsi
	movl	$hello_len, %edx
	call	write1

	movq	(%rbx), %rax		/* argc, one digit is enough here */
	addq	$'0', %rax
	pushq	%rax
	movq	%rsp, %rsi
	movl	$1, %edx
	call	write1
	popq	%rax

	leaq	of(%rip), %rsi
	movl	$of_len, %edx
	call	write1

	movq	8(%rbx), %rsi		/* argv[0] */
	xorl	%edx, %edx
1:	cmpb	$0, (%rsi,%rdx)
	je	2f
	incq	%rdx
	jmp	1b
2:	call	write1

	leaq	nl(%rip), %rsi
	movl	$1, %edx
	call	write1

	movl	$1, %eax		/* SYS_exit */
	xorl	%edi, %edi
	syscall
	ud2

/* write(1, %rsi, %rdx); _exit(1) if it fails (carry flag set). */
write1:
	movl	$4, %eax		/* SYS_write */
	movl	$1, %edi
	syscall
	jc	3f
	ret
3:	movl	$1, %eax
	movl	$1, %edi
	syscall
	ud2
