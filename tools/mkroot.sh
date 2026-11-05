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
#   /sbin      init, mount, sysctl and a few more of the host's;
#   /usr/libexec/getty, /usr/bin/login and the PAM modules login uses;
#   /etc       an rc that mounts tmpfs on /tmp and /var, ttys with a login
#              on the console (ttyu0), a master.passwd of its own (root
#              without a password: nothing is copied from the host's
#              users) and an ordinary user, nova (uid 1001, no password,
#              home /tmp), compiled with pwd_mkdb, login.conf, pam.d, gettytab,
#              termcap;
#   /dev /tmp /var /root, empty.
#
# Next to the tree, <directory>.mtree: an mtree(8) specification with
# every file owned by root, for makefs -F (a user cannot chown).
#
#     /rescue/sh tools/mkroot.sh <directory>
#
# Run with FreeBSD's own shell (/rescue/sh): under the Linuxulator, a
# Linux shell would see the Linux /bin and /lib instead of FreeBSD's.

set -eu
PATH=/rescue:/bin:/usr/bin:/usr/sbin:/usr/local/bin

DEST=${1:?usage: mkroot.sh directory}

mkdir -p "$DEST/rescue" "$DEST/bin" "$DEST/sbin" "$DEST/lib" "$DEST/usr/lib" "$DEST/libexec" \
    "$DEST/usr/libexec" "$DEST/usr/share/misc" "$DEST/etc/pam.d" "$DEST/dev" "$DEST/tmp" "$DEST/var" "$DEST/root"

cp /rescue/rescue "$DEST/rescue/rescue"
for f in $(ls /rescue); do
	[ "$f" = rescue ] || ln "$DEST/rescue/rescue" "$DEST/rescue/$f"
done
SBIN="init mount umount sysctl reboot halt shutdown dmesg nologin"
for f in $SBIN; do
	[ -f "/sbin/$f" ] && cp "/sbin/$f" "$DEST/sbin/"
done
cp /usr/libexec/getty "$DEST/usr/libexec/"

cp /bin/* "$DEST/bin/" 2>/dev/null || true
USR_BIN="login tty who w uptime clear tput awk basename cmp cut dirname du env find grep head id less more printf sed sort tail tee touch tr uname uniq wc which xargs"
mkdir -p "$DEST/usr/bin"
for f in $USR_BIN; do
	[ -f "/usr/bin/$f" ] && cp "/usr/bin/$f" "$DEST/usr/bin/"
done
cp /libexec/ld-elf.so.1 "$DEST/libexec/"

# The shared libraries, and theirs, until no new one appears.
needed() {
	readelf -d "$1" 2>/dev/null | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p'
}
# PAM modules are opened by name (dlopen), not linked: all of them.
cp /usr/lib/pam_*.so.6 "$DEST/usr/lib/"
todo=$(for f in "$DEST"/bin/* "$DEST"/sbin/* "$DEST"/usr/bin/* "$DEST"/usr/libexec/* "$DEST"/usr/lib/pam_*; do
	needed "$f"
done | sort -u)
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

# The system's start: the root is read-only, so what must be written
# goes to memory.
cat > "$DEST/etc/rc" <<'RC'
#!/bin/sh
PATH=/sbin:/bin:/usr/sbin:/usr/bin:/rescue
export PATH
mount -t tmpfs tmpfs /tmp
mount -t tmpfs tmpfs /var
mkdir -p /var/run /var/log /var/tmp /var/db
chmod 1777 /tmp /var/tmp
: > /var/run/utx.active
: > /var/log/utx.log
hostname nova
echo "Nova: /etc/rc done, $(date)"
exit 0
RC

# A login on the console, as the serial line FreeBSD names it.
cat > "$DEST/etc/ttys" <<'TTYS'
console	none				unknown	off secure
ttyu0	"/usr/libexec/getty 3wire.115200"	vt100	on  secure
TTYS

cat > "$DEST/etc/master.passwd" <<'PW'
root::0:0::0:0:Charlie &:/root:/bin/sh
daemon:*:1:1::0:0:Owner of many system processes:/root:/usr/sbin/nologin
nobody:*:65534:65534::0:0:Unprivileged user:/nonexistent:/usr/sbin/nologin
nova::1001:1001::0:0:Nova user:/tmp:/bin/sh
PW
cat > "$DEST/etc/group" <<'GR'
wheel:*:0:root
daemon:*:1:
tty:*:4:
nogroup:*:65533:
nobody:*:65534:
nova:*:1001:
GR
pwd_mkdb -i -p -d "$DEST/etc" "$DEST/etc/master.passwd"
chmod 600 "$DEST/etc/master.passwd"
chmod 700 "$DEST/root"
printf 'group: files\npasswd: files\nhosts: files\n' > "$DEST/etc/nsswitch.conf"
echo /bin/sh > "$DEST/etc/shells"
cp /etc/login.conf /etc/login.access /etc/gettytab "$DEST/etc/"
cap_mkdb "$DEST/etc/login.conf"
for f in login system other; do
	cp "/etc/pam.d/$f" "$DEST/etc/pam.d/"
done
cp /usr/share/misc/termcap /usr/share/misc/termcap.db "$DEST/usr/share/misc/"
ln -s /usr/share/misc/termcap "$DEST/etc/termcap"
# root's shell files, as FreeBSD installs them (its prompt, aliases).
cp /etc/profile "$DEST/etc/"
cp /usr/share/skel/dot.profile "$DEST/root/.profile"
cp /usr/share/skel/dot.shrc "$DEST/root/.shrc"

# Everything belongs to root, whoever built it.
mtree -c -k type,mode,link,size,uid,gid -p "$DEST" |
    sed -e 's/uid=[0-9]*/uid=0/g' -e 's/gid=[0-9]*/gid=0/g' > "$DEST.mtree"
