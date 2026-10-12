Toastrack - multiboot USB sticks for UEFI PCs, with Secure Boot support
=======================================================================

What is in this folder
  toastrack-gui(.exe)   the graphical stick maker (needs administrator rights: Windows asks, on Linux use the button or sudo)
  toastrack(.exe)       the same functions on the command line (toastrack --help)
  bundle/               the boot files that get written to the stick (do not move it away from the programs)
  licenses/             licenses and notices of the redistributed shim

Make a stick
  1. Plug in the USB stick. EVERYTHING ON IT WILL BE ERASED.
  2. Start toastrack-gui, choose the stick, press "Write stick...".
  3. Copy ISO files into the stick's TOASTRACK partition (folder "isos", subfolders are fine), eject it properly.
  4. Boot a PC from the stick. With Secure Boot on, the first time on each PC a blue screen (MokManager) appears:
     "Enroll key from disk" -> ENROLL_THIS_KEY_IN_MOKMANAGER.cer -> confirm -> reboot. The key fingerprint shown there
     must match the one in bundle/VERSION.txt and in the release notes.

Verify this download
  openssl dgst -sha256 -verify toastrack-release-key.pub -signature SHA256SUMS.sig SHA256SUMS   (then sha256sum -c SHA256SUMS)

More: https://github.com/zvillanue/toastrack  (README, docs/USER-GUIDE.md)
