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
# INIT_NAME), ROOTFS=1 (a UFS root file system with the host's /rescue,
# loaded as the memory disk md0, with INIT_PATH, default /rescue/sh, as
# init; INIT is then not used), SINGLE=1 (boot -s: with
# INIT_PATH=/rescue/init, FreeBSD's single-user shell), TIMEOUT=secs
# (stop QEMU after that long; default none), CPU (QEMU CPU model, default
# max), UEFI=1 (boot with QEMU's edk2 firmware and the host's loader.efi
# from an EFI system partition; the console is then the screen, mirrored
# on the serial line), GUI=1 (show the screen in a window; needs a QEMU built
# with a GUI, the "qemu" package, not "qemu-nox11"), VNC=1 (show it over
# VNC on localhost:5900 instead), STAND=1 (boot with the Nova stand
# instead, from a stand.conf written here), LINUX=vmlinuz and
# LINUX_INITRD=initrd (with STAND=1: a Linux entry too), QEMUFLAGS (more
# QEMU options). QEMU exits
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
ROOTFS=${ROOTFS:-}
SINGLE=${SINGLE:-}
INIT_PATH=${INIT_PATH:-}
TIMEOUT=${TIMEOUT:-}
CPU=${CPU:-max}
WORK=${WORK:-sys/target/qemu}
UEFI=${UEFI:-}
AUTOBOOT_DELAY=${AUTOBOOT_DELAY:-1}
STAND=${STAND:-}
LINUX=${LINUX:-}
LINUX_INITRD=${LINUX_INITRD:-}
STAND_EFI=${STAND_EFI:-stand/target/x86_64-unknown-uefi/release/stand.efi}
if [ -n "$STAND" ]; then
	UEFI=1
fi
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
autoboot_delay="$AUTOBOOT_DELAY"
beastie_disable="YES"
console="$CONSOLE"
boot_mute="NO"
CONF
if [ -n "$VERBOSE" ]; then
	echo 'boot_verbose="YES"' >> "$ROOT/boot/loader.conf"
fi
if [ -n "$SINGLE" ]; then
	echo 'boot_single="YES"' >> "$ROOT/boot/loader.conf"
fi
if [ -n "$SELFTEST" ]; then
	echo "debug.nova.selftest=\"$SELFTEST\"" >> "$ROOT/boot/loader.conf"
fi
if [ -n "$INIT" ] && [ -z "$ROOTFS" ]; then
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

if [ -n "$ROOTFS" ]; then
	# The root file system: /rescue as FreeBSD installs it (one crunched
	# binary, hard-linked under every name), /bin and /sbin pointing to
	# it, and the usual empty directories.
	MFS=$WORK/mfsroot
	mkdir -p "$MFS/rescue" "$MFS/dev" "$MFS/etc" "$MFS/tmp" "$MFS/root"
	cp /rescue/rescue "$MFS/rescue/rescue"
	for f in $(ls /rescue); do
		[ "$f" = rescue ] || ln "$MFS/rescue/rescue" "$MFS/rescue/$f"
	done
	ln -s rescue "$MFS/bin"
	ln -s rescue "$MFS/sbin"
	echo "Nova root file system (UFS2 on md0)" > "$MFS/etc/motd"
	makefs -t ffs -o version=2 -f 64 -b 10% "$ROOT/boot/mfsroot" "$MFS" >/dev/null
	cat >> "$ROOT/boot/loader.conf" <<CONF
mfsroot_load="YES"
mfsroot_type="md_image"
mfsroot_name="/boot/mfsroot"
vfs.root.mountfrom="ufs:/dev/md0"
CONF
	echo "init_path=\"${INIT_PATH:-/rescue/sh}\"" >> "$ROOT/boot/loader.conf"
fi

makefs -t ffs -o version=2 -s 128m "$WORK/root.ufs" "$ROOT" >/dev/null
# An EFI system partition with a loader.efi as the default boot program.
# The host's finds /boot on the UFS partition; the Nova stand's reads the
# EFI partition itself (it has no UFS yet), so /boot goes there too.
mkdir -p "$WORK/esp/EFI/BOOT"
if [ -n "$STAND" ]; then
	cp "$STAND_EFI" "$WORK/esp/EFI/BOOT/BOOTX64.EFI"
	cp -R "$ROOT/boot" "$WORK/esp/"
	[ -f /boot/device.hints ] && cp /boot/device.hints "$WORK/esp/boot/"
	# The stand knows no system: stand.conf says what to boot, and how.
	# The last entry chainloads the host's loader.efi (it then boots
	# Nova from the UFS partition).
	cp /boot/loader.efi "$WORK/esp/boot/loader.efi"
	cat > "$WORK/esp/boot/stand.conf" <<CONF
timeout: $AUTOBOOT_DELAY

/Renux Nova
    protocol: freebsd
    path: boot():/boot/kernel/kernel
    comment: Boot the kernel with the settings of loader.conf

/Renux Nova (single-user)
    protocol: freebsd
    path: boot():/boot/kernel/kernel
    cmdline: -s
    comment: Boot to a root shell, without starting services (boot -s)

/Renux Nova (verbose)
    protocol: freebsd
    path: boot():/boot/kernel/kernel
    cmdline: -v
    comment: Boot with every kernel message shown (boot -v)

/FreeBSD loader
    protocol: efi
    path: boot():/boot/loader.efi
    comment: Chainload FreeBSD's loader.efi
CONF
	if [ -n "$LINUX" ]; then
		cp "$LINUX" "$WORK/esp/boot/vmlinuz"
		cat >> "$WORK/esp/boot/stand.conf" <<CONF

/Linux
    protocol: linux
    path: boot():/boot/vmlinuz
    cmdline: console=ttyS0 console=tty0
    comment: Boot Linux through its EFI stub
CONF
		if [ -n "$LINUX_INITRD" ]; then
			cp "$LINUX_INITRD" "$WORK/esp/boot/initrd"
			echo "    module_path: boot():/boot/initrd" >> "$WORK/esp/boot/stand.conf"
		fi
	fi
else
	cp /boot/loader.efi "$WORK/esp/EFI/BOOT/BOOTX64.EFI"
fi
makefs -t msdos -o fat_type=16 -s 64m "$WORK/esp.img" "$WORK/esp" >/dev/null
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
