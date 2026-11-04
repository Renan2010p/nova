# SPDX-License-Identifier: MPL-2.0
#
# Copyright (c) 2026 Renan Lucas Vieira Hilário
#
# This file is part of the Nova Project, the kernel of the
# Renux "Nova" Project.
#
# Top-level Makefile. Portable: works with BSD make and GNU make.
# It only wraps cargo; all real build logic lives in sys/ and tools/.

CARGO?=		cargo
PROFILE?=	release
KERNEL=		sys/target/x86_64-unknown-none/${PROFILE}/kernel
USERLAND=	sys/target/userland
# FreeBSD source tree the ABI tables are generated from.
FREEBSD_SRC?=	/usr/src
# Program preloaded as /sbin/init by `make bhyve`; INIT= for none.
INIT?=		${USERLAND}/hello

all: kernel userland

# Layering rules first: a kernel that violates MD/MI separation is not built.
kernel: check
	cd sys && ${CARGO} build --profile ${PROFILE}
	@echo "==> ${KERNEL}"

# The layering rules, then the kernel built for every port: if MI code
# compiles only for amd64, MD code leaked into it (rule R14).
PORTS=		riscv64gc-unknown-none-elf

check:
	cd tools && ${CARGO} run -q -p checklayers -- ../sys
	@cd sys && for t in ${PORTS}; do \
	    ${CARGO} check -q -p kernel --target $$t || exit 1; \
	    echo "check: the kernel compiles for $$t"; \
	done

# Test programs for the FreeBSD ABI, built with the host's binutils (no
# libc): ${USERLAND}/hello is the default init under bhyve.
userland: ${USERLAND}/hello ${USERLAND}/hello_libc

${USERLAND}/hello: tools/userland/hello.S
	@mkdir -p ${USERLAND}
	as --64 -o ${USERLAND}/hello.o tools/userland/hello.S
	ld -static -nostdlib -z noexecstack -o ${USERLAND}/hello ${USERLAND}/hello.o

# An ordinary static FreeBSD program, with the host's cc and libc (on
# FreeBSD, /usr/bin/clang is the base system's).
${USERLAND}/hello_libc: tools/userland/hello_libc.c
	@mkdir -p ${USERLAND}
	/usr/bin/clang -static -O2 -Wall -o ${USERLAND}/hello_libc tools/userland/hello_libc.c

# Regenerates the FreeBSD ABI's syscall numbers and names from FreeBSD's
# syscalls.master. Run when Nova follows a new FreeBSD release.
sysent:
	cd tools && ${CARGO} run -q -p makesyscalls -- \
	    ${FREEBSD_SRC}/sys/kern/syscalls.master ../sys/compat/freebsd/freebsd_syscalls.rs

# Crates that are plain logic are unit-tested on the host.
test:
	cd sys && ${CARGO} test --target "$$(rustc -vV | sed -n 's/^host: //p')" \
	    -p libkern -p sys -p x86

# Lints, including the explicitness ones (no `as`, no `unwrap`): for the
# kernel, every other port, and the host unit tests.
clippy:
	cd sys && ${CARGO} clippy --profile ${PROFILE}
	@cd sys && for t in ${PORTS}; do \
	    ${CARGO} clippy -q --profile ${PROFILE} -p kernel --target $$t -- -D warnings || exit 1; \
	done
	cd sys && ${CARGO} clippy -q --target "$$(rustc -vV | sed -n 's/^host: //p')" --tests \
	    -p libkern -p sys -p x86 -- -D warnings

# Formal verification, phase 1 (docs/VERIFICATION.md). Neither builds the
# kernel: they check its trusted building blocks on the host.
#
# miri: the host unit tests under Miri, which detects undefined behaviour.
# Needs `rustup toolchain install nightly --component miri,rust-src`.
miri:
	cd sys && ${CARGO} +nightly miri test --target "$$(rustc -vV | sed -n 's/^host: //p')" \
	    -p libkern -p sys -p x86

# kani: the #[kani::proof] harnesses, one at a time, each limited to
# KANI_MEM KiB of memory so a model that grows too big fails instead of
# exhausting the machine. Needs `cargo install kani-verifier` and
# `cargo kani setup`.
KANI_MEM?=	2621440
kani:
	cd sys && ulimit -v ${KANI_MEM} && for p in libkern sys x86; do \
	    ${CARGO} +stable kani -p $$p --output-format terse || exit 1; \
	done

# FreeBSD host only, as root: boots the kernel with the system loader(8).
# VERBOSE=1 boots verbose (boot -v); SELFTEST=pf|stack takes a fatal kernel
# page fault or stack overflow on purpose; INIT=program is preloaded as
# /sbin/init (default: ${USERLAND}/hello; INIT= for none).
# It does not build: run `make` as yourself first, so that the kernel is
# never built as root.
bhyve:
	@test -f ${KERNEL} || { echo "bhyve: ${KERNEL} not found; run make first" >&2; exit 1; }
	VERBOSE="${VERBOSE}" SELFTEST="${SELFTEST}" INIT="${INIT}" \
	    sh tools/bhyve/run.sh ${KERNEL}

# Boots the kernel under QEMU as a real PC (BIOS, gptboot, the host's
# loader(8) from a UFS disk image); no root needed. Same options as bhyve,
# plus TIMEOUT=secs.
qemu:
	@test -f ${KERNEL} || { echo "qemu: ${KERNEL} not found; run make first" >&2; exit 1; }
	CPUS="${CPUS}" VERBOSE="${VERBOSE}" SELFTEST="${SELFTEST}" INIT="${INIT}" TIMEOUT="${TIMEOUT}" \
	    sh tools/qemu/run.sh ${KERNEL}

clean:
	cd sys && ${CARGO} clean
	cd tools && ${CARGO} clean

.PHONY: all kernel userland sysent check test clippy miri kani bhyve qemu clean
