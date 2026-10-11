use std::io::Cursor;
use toastrack_core::{analyze_iso, iso::Iso, SecureBootStatus};

/// Minimal unsigned x86-64 PE32+ image.
fn tiny_pe() -> Vec<u8> {
    let mut d = vec![0u8; 0x400];
    d[..2].copy_from_slice(b"MZ");
    d[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    d[0x80..0x84].copy_from_slice(b"PE\0\0");
    d[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
    d[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes()); // optional header magic
    d
}

fn dir_record(name: &[u8], lba: u32, size: u32, is_dir: bool) -> Vec<u8> {
    let mut len = 33 + name.len();
    if len % 2 == 1 {
        len += 1;
    }
    let mut r = vec![0u8; len];
    r[0] = len as u8;
    r[2..6].copy_from_slice(&lba.to_le_bytes());
    r[10..14].copy_from_slice(&size.to_le_bytes());
    r[25] = if is_dir { 2 } else { 0 };
    r[32] = name.len() as u8;
    r[33..33 + name.len()].copy_from_slice(name);
    r
}

/// Layout: sector16 PVD, 20 root, 21 EFI, 22 BOOT, 23 file data.
fn build_iso(file: &[u8]) -> Vec<u8> {
    let mut img = vec![0u8; 24 * 2048];
    let put = |img: &mut Vec<u8>, sector: usize, recs: Vec<Vec<u8>>| {
        let mut o = sector * 2048;
        for r in recs {
            img[o..o + r.len()].copy_from_slice(&r);
            o += r.len();
        }
    };
    let root = dir_record(&[0], 20, 2048, true);
    img[16 * 2048] = 1;
    img[16 * 2048 + 1..16 * 2048 + 6].copy_from_slice(b"CD001");
    img[16 * 2048 + 156..16 * 2048 + 156 + root.len()].copy_from_slice(&root);
    put(&mut img, 20, vec![dir_record(&[0], 20, 2048, true), dir_record(&[1], 20, 2048, true), dir_record(b"EFI", 21, 2048, true)]);
    put(&mut img, 21, vec![dir_record(b"BOOT", 22, 2048, true)]);
    put(&mut img, 22, vec![dir_record(b"BOOTX64.EFI;1", 23, file.len() as u32, false)]);
    img[23 * 2048..23 * 2048 + file.len()].copy_from_slice(file);
    img
}

#[test]
fn unsigned_loader_is_flagged() {
    let mut iso = Iso::open(Cursor::new(build_iso(&tiny_pe()))).unwrap();
    assert_eq!(analyze_iso(&mut iso).unwrap(), SecureBootStatus::Unsigned);
}

#[test]
fn missing_loader_is_reported() {
    let mut img = build_iso(&tiny_pe());
    img[22 * 2048..23 * 2048].fill(0); // empty the BOOT directory
    let mut iso = Iso::open(Cursor::new(img)).unwrap();
    assert_eq!(analyze_iso(&mut iso).unwrap(), SecureBootStatus::NoUefiLoader);
}

#[test]
fn non_iso_rejected() {
    assert!(Iso::open(Cursor::new(vec![0u8; 40 * 2048])).is_err());
}
