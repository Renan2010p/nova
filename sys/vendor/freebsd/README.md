# FreeBSD vendor code

This directory holds code imported from FreeBSD and the kernel
programming interface (KPI) that lets a FreeBSD driver source port run
on Nova.

```
sys/vendor/freebsd/
├── kpi/            # freebsd_kpi: malloc(9), mtx(9), bus_space, bus_dma...
├── LICENSE         # BSD-2-Clause / BSD-3-Clause notices, as imported
└── README.md
```

`kpi/` is a **service crate**, 100% safe Rust. It is a thin facade over
`framework`, not a second kernel:

* `malloc(9)` → `framework::mm` (`MallocType`, `M_WAITOK`/`M_NOWAIT`,
  `KBox`/`KVec`/`KArc`);
* `mtx(9)` → `framework::sync` (`MtxSpin`, `Mutex`, `WaitQueue`);
* `bus_space`/`bus_dma`/`callout`/`newbus`/`ifnet`/`bio` → planned as
  `framework` gains the device layer.

## Not a `.ko` loader

FreeBSD kernel modules are built against FreeBSD's internal kernel KBI:
symbol table, struct layouts, `sysinit`, `MODULE_DEPEND`, `bus_space_tag_t`,
`if_t`, `bio`, ... Nova implements the FreeBSD **syscall** ABI, not that
KBI. Loading an unmodified `.ko` would be a separate, version-locked and
unsafe project. A port compiles the driver source (Rust, or C behind a
userspace host) against this KPI.

## Licensing

FreeBSD drivers are mostly BSD-2-Clause/BSD-3-Clause, compatible with
Nova's MPL-2.0. Keep the original header and add the file to the
`LICENSE` notice here. Linux drivers are GPL-2.0 and must **not** be
linked into the kernel; they belong in a userspace driver host.

## Rules

* Only drivers and `sys/conf` depend on this crate (R23).
* `sys/compat/*` never depends on `sys/vendor/*` (R22).
* The origin (`freebsd`) is metadata, not an ABI: a driver here does not
  require the FreeBSD userland personality.
