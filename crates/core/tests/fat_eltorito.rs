//! FAT reader against images from mkfs.fat + mtools, and the El Torito fallback against an ISO made by xorriso.
//! Skipped (with a message) when those tools are missing.

use std::{fs, path::Path, process::Command};
use toastrack_core::{
    analyze_iso,
    fat::FatImage,
    io::Shared,
    iso::Iso,
    SecureBootStatus,
};

fn have(t: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {t}")]).output().map(|o| o.status.success()).unwrap_or(false)
}

fn sh(cmd: &str) {
    let st = Command::new("sh").args(["-c", cmd]).env("MTOOLS_SKIP_CHECK", "1").status().unwrap();
    assert!(st.success(), "failed: {cmd}");
}

fn data(seed: u8, len: usize) -> Vec<u8> {
    (0..len).map(|i| (i as u32).wrapping_mul(2654435761).wrapping_add(seed as u32).to_le_bytes()[2]).collect()
}

fn fat_image(dir: &Path, size_mb: u32, fat: &str) -> std::path::PathBuf {
    fat_image_with(dir, size_mb, fat, &data(1, 300_000))
}

/// `efi` becomes /EFI/BOOT/BOOTX64.EFI and the long-named file.
fn fat_image_with(dir: &Path, size_mb: u32, fat: &str, efi: &[u8]) -> std::path::PathBuf {
    let img = dir.join(format!("fat{fat}.img"));
    let big = efi.to_vec();
    fs::write(dir.join("big.bin"), &big).unwrap();
    fs::write(dir.join("small.txt"), b"hello").unwrap();
    sh(&format!(
        "truncate -s {size_mb}M {img:?} && mkfs.fat -F {fat} {img:?} >/dev/null && mmd -i {img:?} ::/EFI ::/EFI/BOOT ::/a-long-directory-name && \
         mcopy -i {img:?} {big:?} ::/EFI/BOOT/BOOTX64.EFI && mcopy -i {img:?} {big:?} '::/a-long-directory-name/A Rather Long File Name.bin' && \
         mcopy -i {img:?} {small:?} ::/small.txt",
        big = dir.join("big.bin"),
        small = dir.join("small.txt"),
    ));
    img
}

#[test]
fn reads_fat12_fat16_fat32() {
    if !(have("mkfs.fat") && have("mcopy")) {
        eprintln!("SKIPPED: mkfs.fat/mtools missing");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for (mb, fat) in [(2, "12"), (20, "16"), (64, "32")] {
        let img = fat_image(dir.path(), mb, fat);
        let mut f = FatImage::open(Shared::new(fs::File::open(&img).unwrap())).unwrap_or_else(|e| panic!("FAT{fat}: {e:?}"));
        for (path, want) in [
            ("EFI/BOOT/BOOTX64.EFI", data(1, 300_000)),
            ("efi/boot/bootx64.efi", data(1, 300_000)),
            ("a-long-directory-name/A Rather Long File Name.bin", data(1, 300_000)),
            ("small.txt", b"hello".to_vec()),
        ] {
            let e = f.find(path).unwrap().unwrap_or_else(|| panic!("FAT{fat}: {path} not found"));
            assert_eq!(f.read_file(&e).unwrap(), want, "FAT{fat}: {path}");
        }
        assert!(f.find("EFI/nope").unwrap().is_none());
        assert!(f.find("small.txt/x").unwrap().is_none());
    }
}

#[test]
fn rejects_non_fat() {
    assert!(FatImage::open(Shared::new(std::io::Cursor::new(vec![0u8; 4096]))).is_err());
}

/// An ISO whose only EFI loader is inside the El Torito image: the analyzer must look there.
#[test]
fn eltorito_fallback_finds_loader_only_in_efi_image() {
    if !(have("mkfs.fat") && have("mcopy") && have("xorriso")) {
        eprintln!("SKIPPED: mkfs.fat/mtools/xorriso missing");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let efi = fat_image_with(dir.path(), 20, "16", &tiny_pe());
    let tree = dir.path().join("tree");
    fs::create_dir_all(tree.join("boot")).unwrap();
    fs::copy(&efi, tree.join("boot/efi.img")).unwrap();
    fs::write(tree.join("README"), b"x").unwrap();
    let iso = dir.path().join("t.iso");
    sh(&format!("xorriso -as mkisofs -r -o {iso:?} --efi-boot boot/efi.img -efi-boot-part --efi-boot-image {tree:?} >/dev/null 2>&1"));

    let mut i = Iso::open(fs::File::open(&iso).unwrap()).unwrap();
    assert!(i.find("EFI/BOOT/BOOTX64.EFI").unwrap().is_none(), "fixture must not have the loader in the file tree");
    assert_eq!(i.efi_boot_images().unwrap().len(), 1);
    // Found inside the FAT image, a valid x86-64 PE without a signature: Unsigned, not "no loader".
    assert_eq!(analyze_iso(&mut i).unwrap(), SecureBootStatus::Unsigned);
    let off = i.efi_boot_images().unwrap()[0];
    let mut raw = vec![0u8; 512];
    i.read_raw(off, &mut raw).unwrap();
    assert_eq!(&raw[510..512], &[0x55, 0xAA]);
}

/// Minimal unsigned x86-64 PE32+ image.
fn tiny_pe() -> Vec<u8> {
    let mut d = vec![0u8; 0x400];
    d[..2].copy_from_slice(b"MZ");
    d[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    d[0x80..0x84].copy_from_slice(b"PE\0\0");
    d[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
    d[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes());
    d
}
