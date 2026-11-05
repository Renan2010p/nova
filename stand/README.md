# Nova stand

The **Nova stand** is the Renux "Nova" bootloader, written in Rust. Like
Limine, it **knows no operating system**: its core reads `stand.conf`,
shows the menu and talks to the firmware; what an entry boots, and how,
is the business of the entry's **protocol**, each a crate of its own under
`proto/` (as the kernel's ABIs live under `compat/`). So the same
`stand.efi` boots the Nova kernel, FreeBSD, Linux, and, by chainloading,
Windows or any other EFI program.

**Status:** under QEMU with UEFI firmware, `stand.efi` boots the Nova
kernel and FreeBSD 15.1's GENERIC kernel (`protocol: freebsd`), Alpine
Linux with its initramfs (`protocol: linux`), and FreeBSD's own
`loader.efi` (`protocol: efi`).

```sh
make stand                                   # stand/target/.../stand.efi
make qemu STAND=1 GUI=1 INIT=...             # writes a stand.conf, boots it
make qemu STAND=1 GUI=1 LINUX=vmlinuz LINUX_INITRD=initrd   # + a Linux entry
```

## stand.conf

Looked for on the volume `stand.efi` was started from, at
`/boot/stand/stand.conf`, `/boot/stand.conf`, `/stand.conf` and
`/EFI/BOOT/stand.conf`. The syntax is Limine's:

```text
timeout: 5                          # seconds; "no" waits; 0 boots at once
default_entry: 1                    # counted from 1
interface_branding: My computer     # the menu's top line

/Renux Nova
    protocol: freebsd
    path: boot():/boot/kernel/kernel
    cmdline: -v
    comment: Boot with every kernel message shown

/Fedora
    protocol: linux
    path: boot():/vmlinuz
    cmdline: root=UUID=... rw quiet
    module_path: boot():/initramfs.img

/Windows
    protocol: efi
    path: boot():/EFI/Microsoft/Boot/bootmgfw.efi
```

| Protocol | Boots | Options | Crate |
|---|---|---|---|
| `freebsd` | a kernel speaking FreeBSD's `loader(8)` protocol: Nova, FreeBSD | `path`, `cmdline` (`boot`'s arguments: `-s`, `-v`, `name=value`) | `proto/freebsd`, `proto/freebsd/arch/amd64/efi` |
| `linux` | Linux, through its EFI stub | `path`, `cmdline`, `module_path` (initrds, concatenated) | `proto/linux` |
| `efi` (`efi_chainload`) | another EFI program: Windows' boot manager, other loaders | `path`, `cmdline` | `proto/chainload` |

Paths are on the boot volume (`boot():/path`, or `/path`). If an entry
cannot boot, or the program it started returns, the error is shown and
the menu comes back.

## Layout

| Directory | Crate | Role |
|---|---|---|
| `efi/` | `efi` | the UEFI interface: console, pages, files of the boot volume, memory map, graphics output, configuration tables, starting another EFI program, an initrd for Linux, `ExitBootServices`. The trusted part: it turns the firmware's raw tables into a safe interface (FreeBSD's `stand/efi/libefi`) |
| `common/` | `common` | the core, 100% safe, no system named: `stand.conf`, the menu, ELF |
| `proto/freebsd/` | `freebsd` | FreeBSD's protocol, MI and 100% safe: the kernel environment, `loader.conf` and `device.hints`, boot flags (`kern/subr_boot.c`) |
| `proto/freebsd/arch/amd64/efi/` | `freebsd_amd64_efi` | the same on amd64 from UEFI: `bi_load`, `elf64_exec`, the trampoline that copies the kernel to 2 MiB and jumps to `btext` (`stand/efi/loader/arch/amd64`) |
| `proto/linux/` | `linux` | Linux's EFI stub: command line, initrd (`LINUX_EFI_INITRD_MEDIA_GUID`) |
| `proto/chainload/` | `chainload` | starting another EFI program |
| `conf/` | `stand` | `stand.efi`: finds `stand.conf`, shows the menu, boots the entry by its protocol |

FreeBSD's protocol is defined once, in the kernel's `sys` crate, and used
by both sides: `sys::linker::PreloadWriter` writes the `MODINFO_*` records
the kernel reads with `sys::linker::Preload`, and the environment is the
format `sys::kenv` reads.

## The boot menu

Laid out as Limine's, drawn by `common::menu` on the firmware's
framebuffer (any `Canvas` would do) as a text terminal in the PC's VGA
font (`sys::font::vga`, 9x16 cells): the branding in cyan on the second
line (`interface_branding`, default "Nova stand 0.1.0 (x86-64, UEFI)"),
the key help in green and gray under it, the entries centred with the
selection in reverse video, and "Booting automatically in N..." near the
bottom; once a key stops the countdown, the selected entry's `comment`
takes its place in cyan. As Limine: Up/Down (wrapping), Home/End move;
Enter, Right or space boot the selection; `1`-`9` boot that entry at
once; `S` restarts into the firmware's setup when the firmware supports
it. Without a framebuffer, the default entry boots.

## What the freebsd protocol passes, as FreeBSD's loader.efi does

* the kernel's `PT_LOAD` segments at their physical addresses (virtual
  minus `KERNBASE`), staged and copied to 2 MiB after `ExitBootServices`;
* the environment: `kernelname`, `device.hints`, `loader.conf` (from
  `/boot` of the boot volume), ACPI's root pointer (`acpi.rsdp`, as
  `acpi_detect`), the `boot_*` flags the console and `cmdline` imply;
* the metadata: `MODINFO_NAME`, `TYPE`, `ADDR`, `SIZE`, and
  `MODINFOMD_ELFHDR`, `HOWTO`, `ENVP`, `KERNEND`, `MODULEP`, `FW_HANDLE`,
  `EFI_ARCH`, `EFI_FB` and `EFI_MAP`;
* `modulep` and `kernend` on the stack, as `btext` reads them.

## Next

Modules for `freebsd` (`module_path`, `*_load`: Nova's `init`, FreeBSD's
`.ko`), other volumes than `boot()` and filesystems the firmware cannot
read (UFS, ext4), sub-entries and an entry editor as Limine's, Linux
kernels without the EFI stub, the BIOS path, and a `checklayers` for the
stand.
