# FreeBSD file systems in Nova

This directory holds file systems whose **format** comes from FreeBSD.
A file system is not an ABI: it is a service that implements
`kern::vnode::VfsOps` and is registered by `sys/conf` with
`vfs_register`. It never depends on `sys/compat/freebsd`.

```
sys/fs/freebsd/
└── ufs/            # UFS2, read-only (FreeBSD's sys/ufs)
```

Conventions:

* one crate per file system, under `sys/fs/<origin>/<name>` (or
  `sys/fs/<name>` for a native Nova format);
* `#![forbid(unsafe_code)]`; every read from the image is checked;
* depends on `framework`, `sys`, `libkern` and (when needed) `kern` for
  the VFS interfaces, never on a personality;
* keep the on-disk format's provenance in the crate docs (for UFS2:
  FreeBSD's `ufs/ffs/fs.h` and `ufs/ufs/dinode.h`);
* `sys/conf` is the only place that names it.

A Linux-origin file system would live in `sys/fs/linux/<name>`, with the
same rules. Rule R19 keeps file system names out of the kernel core;
R21 lets the origin directory use the origin's vocabulary.
