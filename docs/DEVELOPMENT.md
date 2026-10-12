# Development

## Layout
| Crate | What it is |
|---|---|
| `crates/core` (`toastrack-core`) | `no_std` library shared by the loader and the tools: ISO 9660 / El Torito / FAT / exFAT / UDF readers, PE signature parsing and the Secure Boot verdict, GRUB config parsing and distro profiles, checksum helpers, WIM/LZX reader, the Windows hand-off request format |
| `crates/loader` | the UEFI application (`x86_64-unknown-uefi`): menu, virtual optical disk, ISO and kernel boot paths |
| `crates/installer` | partitioning, the exFAT formatter, bundle handling, `install/update/verify` (`ops`), disk access for Linux and Windows (`device`, `winraw`, `sectorio`, `target`) |
| `crates/cli` | the `toastrack` command |
| `crates/gui` | `toastrack-gui`, the graphical stick maker (egui); `model.rs` holds the testable logic; on Windows a manifest asks for administrator rights |
| `crates/winattach` | the Windows helper (`x86_64-pc-windows-gnu`, `no_std`) |

## Building
```sh
rustup target add x86_64-unknown-uefi            # and x86_64-pc-windows-gnu (+ mingw-w64-gcc) for the Windows helper
cargo build -p toastrack                         # CLI (workspace default members exclude the loader and the helper)
cargo build -p toastrack-loader --target x86_64-unknown-uefi --release
cargo build -p toastrack-winattach --target x86_64-pc-windows-gnu --release
```
Release profile: `panic = "abort"`, `opt-level = "s"`, LTO. The loader needs the `.sbat` section (present) or shim refuses it.
Cargo notes: the loader builds SHA-2/SHA-1 with `force-soft` (the UEFI target has no SSE), and defines `wcslen` itself because some
toolchains emit a call to it.

## Tests
`cargo test` runs everything that needs no special tools. Some tests use external reference tools and skip with a message when
they are missing: `exfat_reference` (needs root, `losetup`, `mkfs.exfat`, `mount.exfat-fuse`), the installer's `fsck.exfat`
checks, FAT/El Torito tests (`mtools`, `dosfstools`), `genisoimage -udf` and `wimlib-imagex` for UDF/WIM. A real signed-bundle
install test runs when `TOASTRACK_TEST_BUNDLE` points at a bundle.

## Scripts
| Script | Purpose |
|---|---|
| `make-dev-bundle.sh <dir> <ubuntu.iso> [loader]` | makes a bundle (shim from the ISO, signing key, signed loader, Windows helper if it builds) |
| `make-test-disk.sh`, `qemu.sh` | a test image and QEMU/OVMF runner (`SECBOOT=1 VARS=...` for Secure Boot) |
| `secboot-setup.sh`, `e2e-secboot.sh` | Secure Boot test VM with Microsoft keys and the dev key enrolled; full install-and-boot run |
| `fetch-shim.sh <dir>` | downloads the pinned Ubuntu shim package (`release/shim.lock`), checks it, extracts shim, MokManager and license notices |
| `make-release-key.sh`, `sign-release.sh` | release signing, see [RELEASING.md](RELEASING.md) |
| `build-test-isos.sh` | builds stand-in Debian-live and dracut ISOs from Ubuntu's archive for profile testing |

Secure Boot tests need `qemu-system-x86 ovmf mtools sbsigntool openssl xorriso` and `pip install virt-firmware`.
Driving the menu in a headless QEMU: add `-monitor unix:mon,server,nowait`, use `sendkey` and `screendump`.

## Diagnostics built into the CLI
- `toastrack check-volume <partition> <path>` runs the loader's own exFAT reader and ISO analysis on a real stick, which is how a
  damaged ISO copy was told apart from a reader bug.
- `toastrack explain <iso> [--dump-config]` prints the distro profile and kernel command line for each boot entry.
- `toastrack windows-report <iso>` for Windows ISOs.

## Releasing
Versions are in the crates' `Cargo.toml` (one shared number); tag `vX.Y.Z`; update [CHANGELOG.md](../CHANGELOG.md).
A real release also needs a project signing key and a shim distribution decision, see [ROADMAP.md](ROADMAP.md).

## GUI notes
`cargo build -p toastrack-gui` (not in the workspace default members, so plain `cargo test` stays fast). It needs at runtime X11 or
Wayland, `libxkbcommon` (and `libxkbcommon-x11`), and OpenGL. Headless check: `xvfb-run` with `LIBGL_ALWAYS_SOFTWARE=1` and a screenshot
(`import -window root x.png`). The Windows build cross-compiles with `--target x86_64-pc-windows-gnu` (mingw) and runs under Wine.
`TOASTRACK_TEST_LOADER=<loader.efi> cargo test -p toastrack-gui` additionally writes a stick image through the model.

## Windows disk code
`installer/src/winraw.rs` (physical drives, volume lock/dismount, `IOCTL_DISK_*`) cannot be tested without a disk. What is tested
everywhere: `sectorio.rs` (a Windows raw disk only accepts whole aligned sectors) and `tests/raw_disk_semantics.rs`, which runs
install, update and verify, FAT32 and exFAT, through a file that rejects every unaligned access. When changing the installer's write
patterns, keep that test passing.
