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
# SELFTEST=pf|stack, INIT=program (preloaded as /sbin/init, or as
# INIT_NAME), TIMEOUT=secs
# (stop QEMU after that long; default none), CPU (QEMU CPU model, default
# max), UEFI=1 (boot with QEMU's edk2 firmware and the host's loader.efi
# from an EFI system partition; the console is then the screen, mirrored
# on the serial line), GUI=1 (show the screen in a window; needs a QEMU built
# with a GUI, the "qemu" package, not "qemu-nox11"), VNC=1 (show it over
# VNC on localhost:5900 instead), QEMUFLAGS (more QEMU options). QEMU exits
# when the guest reboots or halts.
#
# To watch the console on the screen:
#
#     make qemu UEFI=1 GUI=1 INIT=sys/target/userland/hello_libc

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
UEFI=${UEFI:-}
GUI=${GUI:-}
VNC=${VNC:-}
EDK2=${EDK2:-/usr/local/share/qemu}

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
if [ -n "$UEFI" ]; then
	# The EFI console only: QEMU's firmware already mirrors it on the
	# serial line, so "comconsole,efi" would print the loader's text twice.
	CONSOLE="efi"
else
	CONSOLE="comconsole"
fi
cat > "$ROOT/boot/loader.conf" <<CONF
autoboot_delay="1"
beastie_disable="YES"
console="$CONSOLE"
boot_mute="NO"
CONF
if [ -n "$VERBOSE" ]; then
	echo 'boot_verbose="YES"' >> "$ROOT/boot/loader.conf"
fi
if [ -n "$SELFTEST" ]; then
	echo "debug.nova.selftest=\"$SELFTEST\"" >> "$ROOT/boot/loader.conf"
fi
if [ -n "$INIT" ]; then
	# INIT_NAME: the path init is loaded as, and so its argv[0] (crunched
	# binaries such as /rescue/rescue choose the program by it).
	INIT_NAME=${INIT_NAME:-/sbin/init}
	mkdir -p "$ROOT$(dirname "$INIT_NAME")"
	cp "$INIT" "$ROOT$INIT_NAME"
	cat >> "$ROOT/boot/loader.conf" <<CONF
nova_init_load="YES"
nova_init_type="nova_exec"
nova_init_name="$INIT_NAME"
init_path="$INIT_NAME"
CONF
fi

makefs -t ffs -o version=2 -s 64m "$WORK/root.ufs" "$ROOT" >/dev/null
# An EFI system partition with the host's loader.efi as the default boot
# program; it finds /boot on the UFS partition.
mkdir -p "$WORK/esp/EFI/BOOT"
cp /boot/loader.efi "$WORK/esp/EFI/BOOT/BOOTX64.EFI"
makefs -t msdos -o fat_type=16 -s 32m "$WORK/esp.img" "$WORK/esp" >/dev/null
mkimg -s gpt -b /boot/pmbr \
	-p efi:="$WORK/esp.img" \
	-p freebsd-boot:=/boot/gptboot \
	-p freebsd-ufs:="$WORK/root.ufs" \
	-o "$WORK/disk.img"

FIRMWARE=
if [ -n "$UEFI" ]; then
	# The variable store must be writable: a copy per run.
	cp "$EDK2/edk2-i386-vars.fd" "$WORK/vars.fd"
	FIRMWARE="-drive if=pflash,format=raw,readonly=on,file=$EDK2/edk2-x86_64-code.fd
	    -drive if=pflash,format=raw,file=$WORK/vars.fd"
fi

if [ -n "$GUI" ]; then
	DISPLAYOPT="-display gtk"
elif [ -n "$VNC" ]; then
	DISPLAYOPT="-display none -vnc 127.0.0.1:0"
	echo "run.sh: screen on VNC at 127.0.0.1:5900"
else
	DISPLAYOPT="-display none"
fi

set -- qemu-system-x86_64 ${QEMUFLAGS:-} $FIRMWARE -machine q35 -cpu "$CPU" \
	-smp "$CPUS" -m "$MEM" $DISPLAYOPT -serial mon:stdio -no-reboot \
	-drive file="$WORK/disk.img",format=raw,if=ide
if [ -n "$TIMEOUT" ]; then
	exec timeout "$TIMEOUT" "$@"
fi
exec "$@"
