#!/usr/bin/env bash
# Build stand-in live ISOs for the distro-profile tests from Ubuntu's archive (no other distro mirrors needed):
#   livedeb.iso  - live-boot based (the initramfs code Debian live and Kali use), grub.cfg only, tiny config
#   livedr.iso   - dracut dmsquash-live based (the code Fedora/RHEL live images use)
# They are NOT official distro images: they prove the kernel parameters and the boot flow, not each distro's own
# configs. Needs root, debootstrap, squashfs-tools, xorriso, e2fsprogs, mtools; ~12 GB of scratch space.
# usage: build-test-isos.sh <work-dir> <ubuntu.iso>   (the Ubuntu ISO supplies the signed shim/GRUB boot image)
set -euo pipefail
work=$1; ubuntu=$2
mkdir -p "$work" && cd "$work"
R=$work/root
debootstrap --variant=minbase --components=main,universe \
  --include=systemd-sysv,udev,kmod,iproute2,initramfs-tools,live-boot,live-boot-initramfs-tools,linux-image-generic \
  noble "$R" http://archive.ubuntu.com/ubuntu
KVER=$(ls "$R"/boot | sed -n 's/^vmlinuz-//p' | head -1)
chroot "$R" passwd -d root >/dev/null
mkdir -p "$R/etc/systemd/system/getty@tty1.service.d"
printf '[Service]\nExecStart=\nExecStart=-/sbin/agetty --autologin root --noclear %%I $TERM\n' > "$R/etc/systemd/system/getty@tty1.service.d/override.conf"
echo TOASTRACK-LIVEBOOT-TEST > "$R/etc/hostname"
# The signed shim + GRUB EFI image from the Ubuntu ISO (Ubuntu's GRUB finds boot/grub/grub.cfg via /.disk/info).
mkdir -p bootimg && xorriso -osirrox on -indev "$ubuntu" -extract_boot_images "$work/bootimg/" 2>/dev/null
mkdir -p ubuntu-grub && xorriso -osirrox on -indev "$ubuntu" -extract /boot/grub/x86_64-efi "$work/ubuntu-grub" 2>/dev/null || true

# ---- live-boot ISO
T=$work/tree-lb; mkdir -p "$T/live" "$T/boot/grub" "$T/.disk"
printf 'echo; echo "*** Toastrack live-boot test system is up ***"; cat /proc/cmdline; findmnt -n /run/live/medium\n' > "$R/root/.profile"
cp "$R/boot/vmlinuz-$KVER" "$T/live/vmlinuz"; cp "$R/boot/initrd.img-$KVER" "$T/live/initrd.img"
mksquashfs "$R" "$T/live/filesystem.squashfs" -comp gzip -e boot -noappend -quiet
cp bootimg/eltorito_img2_uefi.img "$T/boot/grub/efi.img"; echo "Toastrack live-boot test ISO" > "$T/.disk/info"
cp -r ubuntu-grub/x86_64-efi "$T/boot/grub/" 2>/dev/null || true
cat > "$T/boot/grub/grub.cfg" <<'CFG'
set timeout=5
menuentry "Live system" {
    linux /live/vmlinuz boot=live components quiet splash
    initrd /live/initrd.img
}
CFG
xorriso -as mkisofs -r -V TSTRK_LB -J -joliet-long -iso-level 3 -o "$work/livedeb.iso" \
  --efi-boot boot/grub/efi.img -efi-boot-part --efi-boot-image "$T"

# ---- dracut ISO (squashfs.img must contain LiveOS/rootfs.img)
for m in proc sys dev; do mount --bind /$m "$R/$m"; done
chroot "$R" env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq dracut-core dracut-live e2fsprogs
chroot "$R" dracut --no-hostonly --force --add dmsquash-live --omit plymouth --kver "$KVER" /boot/initrd-dracut.img
umount "$R/dev" "$R/sys" "$R/proc"
printf 'echo; echo "*** Toastrack dracut-live test system is up ***"; cat /proc/cmdline; findmnt -n /run/initramfs/live\n' > "$R/root/.profile"
D=$work/tree-dr; mkdir -p "$D/LiveOS" "$D/images/pxeboot" "$D/boot/grub" "$D/.disk" "$work/sq/LiveOS"
cp "$R/boot/vmlinuz-$KVER" "$D/images/pxeboot/vmlinuz"; cp "$R/boot/initrd-dracut.img" "$D/images/pxeboot/initrd.img"; chmod 644 "$D"/images/pxeboot/*
mke2fs -q -t ext4 -L rootfs -d "$R" -E root_owner=0:0 "$work/sq/LiveOS/rootfs.img" 2300M
mksquashfs "$work/sq" "$D/LiveOS/squashfs.img" -comp zstd -Xcompression-level 6 -noappend -quiet
cp bootimg/eltorito_img2_uefi.img "$D/boot/grub/efi.img"; echo "Toastrack dracut test ISO" > "$D/.disk/info"
cp -r ubuntu-grub/x86_64-efi "$D/boot/grub/" 2>/dev/null || true
cat > "$D/boot/grub/grub.cfg" <<'CFG'
set timeout=5
menuentry 'Start Toastrack dracut test live' {
	insmod iso9660
	search --no-floppy --set=root -l 'TSTRK_DR'
	linux /images/pxeboot/vmlinuz root=live:CDLABEL=TSTRK_DR rd.live.image quiet
	initrd /images/pxeboot/initrd.img
}
CFG
xorriso -as mkisofs -r -V TSTRK_DR -J -joliet-long -iso-level 3 -o "$work/livedr.iso" \
  --efi-boot boot/grub/efi.img -efi-boot-part --efi-boot-image "$D"
ls -la "$work"/*.iso
