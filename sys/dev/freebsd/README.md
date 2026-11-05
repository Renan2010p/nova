# FreeBSD drivers in Nova

This directory holds **source-ported FreeBSD drivers**, not binary
FreeBSD `.ko` modules. A driver is a normal Nova service crate:

```
sys/dev/freebsd/
├── e1000/          # one crate per driver
├── ahci/
└── ...
```

Each driver:

* has `#![forbid(unsafe_code)]` and depends only on `framework`, `sys`,
  `libkern` and `sys/vendor/freebsd/kpi` (rule R23);
* implements the bus driver interface (`framework::bus::Driver`, once
  the bus layer lands) and receives `MmioRegion`/`DmaBuffer`/`Irq`
  resources, never raw pointers;
* is registered by `sys/conf` (built-in) or by the module loader;
* never depends on `sys/compat/freebsd` (the userland ABI) or on any
  other personality. A driver is hardware, not an ABI;
* keeps the original BSD-2-Clause/BSD-3-Clause header and the upstream
  copyright notice.

The KPI that makes the port small is
[`sys/vendor/freebsd/kpi`](../../vendor/freebsd/kpi): it provides
`malloc(9)`, `mtx(9)` and, as the framework grows, `bus_space`,
`bus_dma`, `callout` and `newbus`.

**Status:** empty on purpose. The first port waits for the framework's
device layer (`bus_space`, `bus_dma`, PCI, IRQ), milestone 7. The first
native driver will be `virtio` in `sys/dev/virtio`, which proves the
model without imported code; `e1000`/`ahci` can follow here.

## Why not `sys/compat/freebsd/dev/`?

`compat/freebsd` is the **userland ABI** (syscalls, signals, exec). A
driver is not an ABI: the same `e1000` works with a FreeBSD, Linux or
native Nova userland. Keeping drivers here preserves that separation;
rule R22 forbids a personality from depending on a driver or KPI.

## Adding a driver

1. Create `sys/dev/freebsd/<name>/` with `Cargo.toml` and `lib.rs`.
2. Add it to `members` and `[workspace.dependencies]` in
   `sys/Cargo.toml`.
3. Implement the driver interface and register it from `sys/conf`.
4. Keep `unsafe` out: if a C source port is unavoidable, discuss it
   first; it is not a safe service and belongs behind the userspace
   driver host, not here.
