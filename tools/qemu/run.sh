#!/bin/sh
#
# SPDX-License-Identifier: MPL-2.0
#
# Copyright (c) 2026 Renan Lucas Vieira Hilário
#
# This file is part of the Nova Project, the kernel of the
# Renux "Nova" Project.
#
# Boots a Nova kernel under QEMU, as a real PC would: BIOS, the host's
# pmbr and gptboot, then the host's FreeBSD loader(8) from a UFS
# partition. No root is needed: the disk image is made with makefs(8) and
# mkimg(1). The console is the serial port, on this terminal.
#
#     sh tools/qemu/run.sh sys/target/x86_64-unknown-none/release/kernel
#
# Environment: MEM (default 512M), CPUS (default 2), VERBOSE=1 (boot -v),
# SELFTEST=pf|stack, INIT=program (preloaded as /sbin/init), TIMEOUT=secs
# (stop QEMU after that long; default none), CPU (QEMU CPU model, default
# max). QEMU exits when the guest reboots or halts.

set -eu

KERNEL=${1:-sys/target/x86_64-unknown-none/release/kernel}
MEM=${MEM:-512M}
CPUS=${CPUS:-2}
VERBOSE=${VERBOSE:-}
SELFTEST=${SELFTEST:-}
INIT=${INIT:-}
TIMEOUT=${TIMEOUT:-}
CPU=${CPU:-max}
WORK=${WORK:-sys/target/qemu}

if [ ! -f "$KERNEL" ]; then
	echo "run.sh: kernel not found: $KERNEL (run make first)" >&2
	exit 1
fi

rm -rf "$WORK"
ROOT=$WORK/root
mkdir -p "$ROOT/boot/kernel"

# The loader, its scripts and defaults from the host; Nova as the kernel.
cp /boot/loader "$ROOT/boot/"
cp -R /boot/lua /boot/defaults "$ROOT/boot/"
cp "$KERNEL" "$ROOT/boot/kernel/kernel"
cat > "$ROOT/boot/loader.conf" <<CONF
autoboot_delay="1"
beastie_disable="YES"
console="comconsole"
boot_mute="NO"
CONF
if [ -n "$VERBOSE" ]; then
	echo 'boot_verbose="YES"' >> "$ROOT/boot/loader.conf"
fi
if [ -n "$SELFTEST" ]; then
	echo "debug.nova.selftest=\"$SELFTEST\"" >> "$ROOT/boot/loader.conf"
fi
if [ -n "$INIT" ]; then
	mkdir -p "$ROOT/sbin"
	cp "$INIT" "$ROOT/sbin/init"
	cat >> "$ROOT/boot/loader.conf" <<CONF
nova_init_load="YES"
nova_init_type="nova_exec"
nova_init_name="/sbin/init"
CONF
fi

makefs -t ffs -o version=2 -s 64m "$WORK/root.ufs" "$ROOT" >/dev/null
mkimg -s gpt -b /boot/pmbr \
	-p freebsd-boot:=/boot/gptboot \
	-p freebsd-ufs:="$WORK/root.ufs" \
	-o "$WORK/disk.img"

set -- qemu-system-x86_64 ${QEMUFLAGS:-} -machine q35 -cpu "$CPU" -smp "$CPUS" -m "$MEM" \
	-display none -serial mon:stdio -no-reboot \
	-drive file="$WORK/disk.img",format=raw,if=ide
if [ -n "$TIMEOUT" ]; then
	exec timeout "$TIMEOUT" "$@"
fi
exec "$@"
