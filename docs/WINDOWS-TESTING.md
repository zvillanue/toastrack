# Testing Windows ISOs (needs a Windows ISO, which the development sandbox does not have)

Nothing about Windows has been boot-tested yet. These steps produce the facts needed to build the real fix
(see "Windows ISOs" in DESIGN.md). Use a Windows 10 or 11 ISO you are licensed to have; nothing below uploads it.

## 1. The report (2 minutes, no stick needed)
    cargo run -p toastrack -- windows-report Win11_23H2_x64.iso --out report.txt
Paste `report.txt`. It contains names, sizes, WIM header fields, image names/builds and the boot manager's signature
data (signer names, authenticode hash) - never file contents. What it tells me:
- whether the ISO's UDF tree parses (a UDF feature I do not support is reported, not guessed),
- the exact boot manager certificate names (my "this is the Windows boot manager" check was written from knowledge of
  the 2011 PCA / 2023 CA naming and has never seen a real Windows binary),
- whether `boot.wim` has two images (WinPE + Setup) and how it is compressed (LZX / XPRESS),
- whether `install.wim` is over 4 GiB (does not fit FAT32 unsplit) or an `.esd`.

### 1b. Second report, after `git pull` (looks inside boot.wim)
    cargo run -p toastrack -- windows-report Win11_25H2_English_x64.iso --out report2.txt \
        --show-file Windows/System32/winpeshl.ini --show-file Windows/System32/startnet.cmd
This now also decompresses the metadata of each image in `boot.wim` (LZX, SHA-1 verified against the WIM's own record),
lists its top-level layout, the files a launcher would hook into, and the `sources/` folder of the Setup image.
`--show-file` additionally prints the named small text files (Microsoft's launch configuration, a few lines each);
leave it out if you would rather not share them. It tells me how WinPE and Setup start, i.e. exactly what to change.

## 2. What the current loader does with it
Make a stick (`toastrack install`, then copy the ISO into the `isos` folder), boot a PC or QEMU with it, open the ISO.
Expected today: the ISO is tagged `WIN`, only "Boot the ISO's own bootloader" is offered. Choose it and note, for
Secure Boot **on** and **off** (and which PC / firmware):
1. Does the Windows boot manager start (blue Windows logo, "Press any key" prompt)? A message like
   "Security Violation" or an `ACCESS_DENIED` text means the boot manager was refused: say which.
2. Does WinPE start (language/keyboard screen)?
3. Does Setup get past "Install now" to the disk selection, or does it stop with "A media driver your computer needs is
   missing" / cannot find install files? (Expected: it cannot find them, because the virtual disk is invisible to WinPE.)
A phone photo of each screen is plenty.

## 3. QEMU instead of a PC (if you have a Linux box)
    scripts/qemu.sh stick.img -m 4096 -accel kvm     # SECBOOT=1 VARS=... for Secure Boot, see scripts/e2e-secboot.sh
Windows setup needs about 4 GB of RAM and a USB or SATA disk; `-accel kvm` makes it usable.

## 4. What I will do with the results
Approach A in DESIGN.md: patch the RAM-loaded `boot.wim` so WinPE runs a small helper that finds the ISO file on the
stick, attaches it with the Windows virtual-disk API and starts Setup from it. The report's WIM facts fix the
compression/format work; the screenshots tell me whether the boot manager and WinPE stages already behave.
