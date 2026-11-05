#!/bin/sh
#
# SPDX-License-Identifier: MPL-2.0
#
# Copyright (c) 2026 Renan Lucas Vieira Hilário
#
# This file is part of the Nova Project, the kernel of the
# Renux "Nova" Project.
#
# Builds a FreeBSD root file system tree from the host, for Nova to boot
# from as its memory disk (make qemu ROOTFS=1, make bhyve ROOTFS=1):
#
#   /rescue    as FreeBSD installs it: one crunched binary, hard-linked
#              under every name;
#   /bin       the host's, dynamically linked;
#   /usr/bin   a few of the host's everyday tools (USR_BIN below);
#   /libexec/ld-elf.so.1, and every shared library /bin needs, found
#              with readelf(1) in /lib and /usr/lib;
#   /sbin      pointing to /rescue (for now);
#   /dev /etc /tmp /root, empty but for /etc/motd.
#
#     /rescue/sh tools/mkroot.sh <directory>
#
# Run with FreeBSD's own shell (/rescue/sh): under the Linuxulator, a
# Linux shell would see the Linux /bin and /lib instead of FreeBSD's.

set -eu
PATH=/rescue:/bin:/usr/bin:/usr/local/bin

DEST=${1:?usage: mkroot.sh directory}

mkdir -p "$DEST/rescue" "$DEST/bin" "$DEST/lib" "$DEST/usr/lib" "$DEST/libexec" \
    "$DEST/dev" "$DEST/etc" "$DEST/tmp" "$DEST/root"

cp /rescue/rescue "$DEST/rescue/rescue"
for f in $(ls /rescue); do
	[ "$f" = rescue ] || ln "$DEST/rescue/rescue" "$DEST/rescue/$f"
done
ln -s rescue "$DEST/sbin"

cp /bin/* "$DEST/bin/" 2>/dev/null || true
USR_BIN="awk basename cut dirname du env find grep head id less more printf sed sort tail tee tr uname uniq wc which xargs"
mkdir -p "$DEST/usr/bin"
for f in $USR_BIN; do
	[ -f "/usr/bin/$f" ] && cp "/usr/bin/$f" "$DEST/usr/bin/"
done
cp /libexec/ld-elf.so.1 "$DEST/libexec/"

# The shared libraries, and theirs, until no new one appears.
needed() {
	readelf -d "$1" 2>/dev/null | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p'
}
todo=$(for f in "$DEST"/bin/* "$DEST"/usr/bin/*; do needed "$f"; done | sort -u)
while [ -n "$todo" ]; do
	next=
	for lib in $todo; do
		for dir in lib usr/lib; do
			if [ -f "/$dir/$lib" ] && [ ! -f "$DEST/$dir/$lib" ]; then
				cp "/$dir/$lib" "$DEST/$dir/$lib"
				next="$next $(needed "/$dir/$lib")"
				break
			fi
		done
	done
	todo=$(echo $next | tr ' ' '\n' | sort -u)
done

echo "Nova root file system (UFS2 on md0)" > "$DEST/etc/motd"
