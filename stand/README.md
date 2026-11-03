# Nova stand

**Not started yet.** This directory is reserved for the **Nova stand**,
the Rust implementation of the Renux "Nova" bootloader, which will replace
FreeBSD's `stand/` (`loader(8)`, `loader.efi`, `boot1`/`gptboot`, ...).

## Why it can wait

The Nova kernel speaks the **`loader(8)` boot protocol**: entry at `btext`
in long mode, `modulep`/`kernend` on the stack, and *preload* metadata
(`MODINFO_*`). See [docs/BOOT.md](../docs/BOOT.md). Until the Nova stand
exists, FreeBSD's own `loader(8)` loads Nova with no changes at all.

## Contract

The Nova stand must be **drop-in in both directions**:

1. load both the Nova kernel **and** the FreeBSD kernel, using the same
   protocol: the same `modulep`, the same `MODINFO_*` and `MODINFOMD_*`
   records and the same environment (`envp`);
2. read the same configuration: `/boot/loader.conf`,
   `/boot/loader.conf.d/`, environment variables, `boot -s`/`-v`
   (`boothowto`).

That keeps kernel and bootloader interchangeable throughout the Renux
transition.

## Sketch of the layout (FreeBSD `stand/` style)

```
stand/
├── common/     # MI: ELF loader, preload metadata, environment, loader.conf
├── efi/        # loader.efi (UEFI)
├── i386/       # BIOS: boot0, gptboot, loader (BIOS)
├── userboot/   # for bhyveload
└── libsa/      # the bootloader's mini libc (in Rust: no_std)
```

The boot protocol is already defined in code in the kernel:
`sys/sys/linker.rs` (MI) and `sys/arch/amd64/metadata.rs` (MD). The Nova
stand will reuse those definitions instead of duplicating them.
