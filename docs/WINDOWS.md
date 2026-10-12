# Windows ISOs (experimental)

**Status in 0.1.0:** a Windows 11 25H2 ISO started from the Toastrack menu in VirtualBox (UEFI, Secure Boot **off**) reached
Setup's language page and then the disk selection page, which means Setup found `install.wim`. No installation was completed,
Secure Boot **on** was not tried with Windows, and no real PC was tried. Treat it as a preview.

## The problem and how it is solved
Toastrack starts an ISO by exposing it to the firmware as a virtual disk. That works for the Windows boot manager and for
`boot.wim` (loaded into RAM), but once WinPE runs, the virtual disk is gone, so Setup cannot find `sources\install.wim`.
Windows ISOs are also UDF (their ISO 9660 side is a one-file stub) and `install.wim` is usually over 4 GB.

Toastrack therefore puts three things in play:
1. When you boot a `WIN` ISO, the loader stores the ISO's location (the stick's volume serial number and the file path) in a
   UEFI variable `ToastrackIso`.
2. The stick's data partition root holds `autounattend.xml`. Setup reads that file from removable media and, in its early
   (windowsPE) phase, runs one command: `toastrack\attach.exe` from whichever drive has it. Nothing else in Setup is automated.
3. `attach.exe` (`crates/winattach`, a small `no_std` program that imports only `kernel32`, `advapi32`, `msvcrt` and
   `virtdisk`) reads the variable, finds the stick by serial number, and attaches the ISO as a read-only virtual DVD with the
   Windows virtual-disk API. Setup then finds `sources\install.wim` on that DVD.

No Microsoft file is modified, so the Windows boot manager's signature stays intact. The helper writes `toastrack\attach.log` on
the stick, which records each step. A successful run looks like:
```
request: volume serial 54238DA2, ISO /isos/Win11_25H2_English_x64.iso
data volume is C:
ISO found: C:\isos\Win11_25H2_English_x64.iso (7736125440 bytes)
ISO attached; waiting for its drive letter
install media is now E:
```
`toastrack`, `autounattend.xml` and system folders are hidden in the menu's root listing.

## Using it
1. Build the helper (`rustup target add x86_64-pc-windows-gnu`, `mingw-w64-gcc`, then
   `cargo build --release -p toastrack-winattach --target x86_64-pc-windows-gnu`). `scripts/make-dev-bundle.sh` adds it to
   the bundle as `toastrack-attach.exe` when it builds.
2. `toastrack install ... --bundle <dir>` (or `--attach-exe path\to\toastrack-attach.exe` with `--unsigned-loader`) puts the
   helper and `autounattend.xml` on the data partition. A stick without the helper still starts Windows ISOs, but Setup then
   cannot find its install files.
3. Copy the Windows ISO into `isos/`, boot it from the menu (tag `WIN`), choose the chainload entry.

Things to know:
- Setup searches the root of every removable drive for `autounattend.xml`, so with the stick plugged in while installing
  Windows from other media, Setup runs the helper too. It exits harmlessly when no request is stored.
- The disk list in Setup shows the stick's own partitions (`TOASTRACK-ESP`, `TOASTRACK`). Do not pick them.
- Media signed only by the 2011 Windows PCA may be refused by PCs that have applied the boot-manager revocations (BlackLotus
  mitigations); newer media uses "Windows UEFI CA 2023", which older firmware does not trust yet. The badge cannot predict this.
- Windows PE or rescue ISOs (everything inside `boot.wim`) need no helper.

## What is unproven
- Secure Boot **on** with a Windows ISO. The Windows boot manager is signed by a Microsoft Windows CA that stock firmware
  trusts, so it should load, but this was not tried.
- Real hardware (USB visibility in WinPE, firmware variable access from WinPE).
- A complete installation, Windows 10, ISOs with only `install.esd` (the WIM reader handles LZX; ESD uses LZMS/solid
  compression which it does not).

## Reporting results (please do)
1. `toastrack windows-report Win11.iso --out report.txt` describes the ISO: layout, boot manager signature, `boot.wim` and
   `install.wim` images. It contains names, sizes and signature data, never file contents (`--show-file` adds the content of a
   named small text file, off by default). Safe to paste into an issue.
2. Boot the ISO from a stick and note, with phone photos if easy: does the Windows boot manager start; does Setup show its
   language page; does it reach disk selection or complain about missing media/drivers?
3. Attach `toastrack\attach.log` from the stick (it shows the Windows error code if attaching failed) and say whether Secure
   Boot was on, the PC or VM model, and the firmware.

## Notes for developers
- The UDF reader (`core::udf`) handles the physical partition only; UDF 2.50 metadata/virtual/sparable maps are reported as
  unsupported, not guessed.
- `core::wim` and `core::lzx` read WIM headers, XML, lookup tables, metadata and directory trees, with LZX (32 KiB to 2 MiB
  windows) verified against wimlib-made WIMs and a real Windows 11 `boot.wim`. A bug found on the way: in the pretree
  coding, code 19 gives a whole run the same new length computed from the first entry's previous length.
- Why not patch `boot.wim` as Ventoy does: it would need a WIM writer and keeps working only while Microsoft's image layout
  does. The answer-file route changes no Microsoft file. Rufus-style file extraction would need FAT32 with split WIMs or a
  filesystem driver shim and breaks "drop the ISO in".
- The loader's `force-windows-handoff` cargo feature (test only) makes every ISO publish the Windows request, to test the
  hand-off without Windows media.
