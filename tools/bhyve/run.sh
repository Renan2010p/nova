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
# SELFTEST (e.g. "pf": take a kernel page fault on purpose, to check that
# traps are caught and reported).

set -eu

KERNEL=${1:-sys/target/x86_64-unknown-none/release/kernel}
VM=${VM:-nova}
MEM=${MEM:-512M}
CPUS=${CPUS:-2}
SELFTEST=${SELFTEST:-}

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
2) echo "run.sh: guest halted (expected for Nova today)" ;;
3) echo "run.sh: guest triple-faulted" ;;
*) echo "run.sh: bhyve exited with error $rc" ;;
esac
