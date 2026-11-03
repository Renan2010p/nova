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

all: kernel

# Layering rules first: a kernel that violates MD/MI separation is not built.
kernel: check
	cd sys && ${CARGO} build --profile ${PROFILE}
	@echo "==> ${KERNEL}"

check:
	cd tools && ${CARGO} run -q -p checklayers -- ../sys

# Crates that are plain logic are unit-tested on the host.
test:
	cd sys && ${CARGO} test --target "$$(rustc -vV | sed -n 's/^host: //p')" \
	    -p libkern -p sys -p x86

clippy:
	cd sys && ${CARGO} clippy --profile ${PROFILE}

# FreeBSD host only, as root: boots the kernel with the system loader(8).
# VERBOSE=1 boots verbose (boot -v); SELFTEST=pf|stack takes a fatal kernel
# page fault or stack overflow on purpose.
bhyve: kernel
	VERBOSE="${VERBOSE}" SELFTEST="${SELFTEST}" sh tools/bhyve/run.sh ${KERNEL}

clean:
	cd sys && ${CARGO} clean
	cd tools && ${CARGO} clean

.PHONY: all kernel check test clippy bhyve clean
