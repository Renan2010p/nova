#!/bin/sh
#
# SPDX-License-Identifier: MPL-2.0
#
# Copyright (c) 2026 Renan Lucas Vieira Hilário
#
# This file is part of the Nova Project, the kernel of the
# Renux "Nova" Project.
#
# Boots a Nova kernel under bhyve(8), loaded by the host's own FreeBSD
# loader (userboot, through bhyveload(8)). No disk image is needed: the
# loader reads a temporary directory that looks like /boot.
#
# Run on a FreeBSD host, as root, from a native shell (not from inside the
# Linuxulator):
#
#     doas sh tools/bhyve/run.sh sys/target/x86_64-unknown-none/release/kernel
#
# Environment: VM (default nova), MEM (default 512M), CPUS (default 2),
# VERBOSE=1 (boot_verbose, like "boot -v": diagnostics and self-test
# results), SELFTEST=pf|stack (a fatal kernel page fault or stack overflow
# on purpose, to check that traps are caught and reported), INIT=program
# (preloaded as /sbin/init, which Nova executes as process 1, when there
# is no root file system), ROOTFS=1 (a UFS root file system holding the
# host's /rescue, loaded as the memory disk md0, with INIT_PATH, default
# /rescue/sh, as init; INIT is then not used), SINGLE=1 (boot -s: with
# INIT_PATH=/rescue/init, FreeBSD's single-user shell).
#
# Nova halts like FreeBSD: "Please press any key to reboot." Press a key
# (or wait for the panic countdown) and bhyve exits.

set -eu

KERNEL=${1:-sys/target/x86_64-unknown-none/release/kernel}
VM=${VM:-nova}
MEM=${MEM:-512M}
CPUS=${CPUS:-2}
SELFTEST=${SELFTEST:-}
VERBOSE=${VERBOSE:-}
INIT=${INIT:-}
INIT_NAME=${INIT_NAME:-}
ROOTFS=${ROOTFS:-}
SINGLE=${SINGLE:-}
INIT_PATH=${INIT_PATH:-}

if [ "$(uname -s)" != FreeBSD ]; then
	echo "run.sh: bhyve needs a FreeBSD host" >&2
	exit 1
fi
if [ "$(id -u)" -ne 0 ]; then
	echo "run.sh: bhyve needs root (try: doas sh $0 $*)" >&2
	exit 1
fi
if [ ! -f "$KERNEL" ]; then
	echo "run.sh: kernel not found: $KERNEL (run make first)" >&2
	exit 1
fi

kldstat -q -m vmm || kldload vmm

ROOT=$(mktemp -d -t nova-bhyve)
cleanup() {
	bhyvectl --destroy --vm="$VM" >/dev/null 2>&1 || true
	rm -rf "$ROOT"
}
trap cleanup EXIT INT TERM

# A minimal /boot: the loader's scripts and defaults from the host, and
# Nova in place of the FreeBSD kernel. boot_mute is forced off because the
# host's defaults may set it (Renux does), and we want to see the output.
mkdir -p "$ROOT/boot/kernel"
cp -R /boot/lua /boot/defaults "$ROOT/boot/"
cp "$KERNEL" "$ROOT/boot/kernel/kernel"
cat > "$ROOT/boot/loader.conf" <<EOF
autoboot_delay="1"
beastie_disable="YES"
console="comconsole"
boot_mute="NO"
EOF
if [ -n "$VERBOSE" ]; then
	echo 'boot_verbose="YES"' >> "$ROOT/boot/loader.conf"
fi
if [ -n "$SINGLE" ]; then
	echo 'boot_single="YES"' >> "$ROOT/boot/loader.conf"
fi
if [ -n "$INIT" ] && [ -z "$ROOTFS" ]; then
	# INIT_NAME: the path init is loaded as, and so its argv[0] (crunched
	# binaries such as /rescue/rescue choose the program by it).
	INIT_NAME=${INIT_NAME:-/sbin/init}
	mkdir -p "$ROOT$(dirname "$INIT_NAME")"
	cp "$INIT" "$ROOT$INIT_NAME"
	cat >> "$ROOT/boot/loader.conf" <<EOF
nova_init_load="YES"
nova_init_type="nova_exec"
nova_init_name="$INIT_NAME"
init_path="$INIT_NAME"
EOF
fi
if [ -n "$ROOTFS" ]; then
	# The root file system: the host's /rescue, /bin and the libraries
	# they need (tools/mkroot.sh).
	MFS=$ROOT/mfsroot
	/rescue/sh "$(dirname "$0")/../mkroot.sh" "$MFS"
	makefs -t ffs -o version=2 -f 64 -b 10% "$ROOT/boot/mfsroot" "$MFS" >/dev/null
	cat >> "$ROOT/boot/loader.conf" <<EOF
mfsroot_load="YES"
mfsroot_type="md_image"
mfsroot_name="/boot/mfsroot"
vfs.root.mountfrom="ufs:/dev/md0"
init_path="${INIT_PATH:-/rescue/sh}"
EOF
fi
if [ -n "$SELFTEST" ]; then
	echo "debug.nova.selftest=\"$SELFTEST\"" >> "$ROOT/boot/loader.conf"
fi

bhyvectl --destroy --vm="$VM" >/dev/null 2>&1 || true
bhyveload -m "$MEM" -h "$ROOT" -c stdio "$VM"

set +e
# -x: offer x2APIC to the guest; Nova needs it (TODO(xapic)).
bhyve -c "$CPUS" -m "$MEM" -H -P -A -x \
	-s 0,hostbridge -s 31,lpc -l com1,stdio \
	"$VM"
rc=$?
set -e

# bhyve(8) exit status.
case $rc in
0) echo "run.sh: guest rebooted" ;;
1) echo "run.sh: guest powered off" ;;
2) echo "run.sh: guest halted" ;;
3) echo "run.sh: guest triple-faulted" ;;
*) echo "run.sh: bhyve exited with error $rc" ;;
esac
