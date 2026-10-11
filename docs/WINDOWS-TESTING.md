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

## 2. Stick test of the Windows Setup hand-off (unverified design)
Setup cannot see the firmware's virtual disk, so the loader stores the ISO's location in a UEFI variable
(`ToastrackIso`), and the stick carries an `autounattend.xml` at its root that makes Setup run `toastrack\attach.exe`
before it looks for install files. The helper reads the variable, finds the stick, and attaches the ISO file as a virtual
DVD with the Windows virtual-disk API; Setup then finds `sources\install.wim` there. Nothing patches Microsoft's files.

Build on Arch (Secure Boot **off** for the first test):

    sudo pacman -S mingw-w64-gcc
    rustup target add x86_64-pc-windows-gnu x86_64-unknown-uefi
    git pull
    cargo build -p toastrack-loader --target x86_64-unknown-uefi --release
    cargo build -p toastrack-winattach --target x86_64-pc-windows-gnu --release
    cargo run -p toastrack -- list                      # find your USB stick - double check the device!
    cargo run -p toastrack -- install /dev/sdX --unsigned-loader target/x86_64-unknown-uefi/release/toastrack-loader.efi \
        --attach-exe target/x86_64-pc-windows-gnu/release/toastrack-attach.exe

`install` ERASES the whole device. Copy the Windows ISO into the `isos` folder of the TOASTRACK partition, boot the PC
from the stick, open the ISO (tag `WIN`), choose the chainload entry, and note at each stage (phone photos are fine):
1. Does the Windows boot manager start? 2. Does WinPE/Setup start (language screen)? 3. Does Setup reach disk selection,
or does it say a driver/install media is missing? Afterwards send `toastrack\attach.log` from the stick: it records
what the helper found and the exact Windows error code if the attach failed. The `toastrack` folder is hidden in the menu.

## 3. QEMU instead of a PC (if you have a Linux box)
    scripts/qemu.sh stick.img -m 4096 -accel kvm     # SECBOOT=1 VARS=... for Secure Boot, see scripts/e2e-secboot.sh
Windows setup needs about 4 GB of RAM and a USB or SATA disk; `-accel kvm` makes it usable.

## 4. What I will do with the results
The attach.log and photos show which assumption failed (answer file not read, USB volume invisible, variable unreadable,
virtdisk refusing the ISO), and the fix follows from that.
