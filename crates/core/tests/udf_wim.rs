//! UDF reader and WIM parser against images made by genisoimage (-udf) and wimlib-imagex.
//! Skipped (with a message) when those tools are missing.

use std::{fs, path::Path, process::Command};
use toastrack_core::{
    udf::{UdfFileReader, UdfVolume},
    wim,
};

fn have(t: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {t}")]).output().map(|o| o.status.success()).unwrap_or(false)
}

fn sh(cmd: &str) {
    let st = Command::new("sh").args(["-c", cmd]).status().unwrap();
    assert!(st.success(), "failed: {cmd}");
}

fn data(seed: u32, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2654435761) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 8) as u8
        })
        .collect()
}

fn write(p: &Path, b: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b).unwrap();
}

/// A Windows-shaped tree: bootmgr, boot/, efi/, sources/boot.wim (2 images) and sources/install.wim.
fn build(dir: &Path) -> Option<std::path::PathBuf> {
    if !(have("genisoimage") && have("wimlib-imagex")) {
        eprintln!("SKIPPED: genisoimage / wimlib-imagex missing");
        return None;
    }
    let t = dir.join("tree");
    write(&t.join("bootmgr"), &data(1, 400_000));
    write(&t.join("boot/bcd"), &data(2, 20_000));
    write(&t.join("boot/boot.sdi"), &data(3, 3_000_000));
    write(&t.join("efi/boot/bootx64.efi"), &data(4, 1_200_000));
    write(&t.join("efi/microsoft/boot/bcd"), &data(5, 20_000));
    write(&t.join("sources/setup.exe"), &data(6, 100_000));
    write(&t.join("sources/empty.txt"), b"");
    write(&t.join("sources/h\u{e9}llo w\u{f6}rld.txt"), b"unicode");
    for i in 0..200 {
        write(&t.join(format!("sources/lang/a-fairly-long-file-name-number-{i:04}.dat")), format!("{i}").as_bytes());
    }
    // Two WIM images in boot.wim (WinPE + Setup) and one in install.wim.
    for (n, seed) in [("pe", 10u32), ("setup", 11)] {
        write(&dir.join(format!("img-{n}/windows/system32/winpe.txt")), &data(seed, 300_000));
        write(&dir.join(format!("img-{n}/sources/{n}.bin")), &data(seed + 100, 150_000));
    }
    write(&dir.join("img-os/windows/system32/os.bin"), &data(20, 800_000));
    sh(&format!(
        "wimlib-imagex capture {pe} {boot} 'Microsoft Windows PE (x64)' 'Windows PE' --compress=LZX >/dev/null && \
         wimlib-imagex append {setup} {boot} 'Microsoft Windows Setup (x64)' 'Windows Setup' >/dev/null && \
         wimlib-imagex capture {os} {inst} 'Windows 11 Pro' 'Windows 11 Pro' --compress=LZX >/dev/null",
        pe = dir.join("img-pe").display(),
        setup = dir.join("img-setup").display(),
        os = dir.join("img-os").display(),
        boot = t.join("sources/boot.wim").display(),
        inst = t.join("sources/install.wim").display(),
    ));
    let iso = dir.join("win.iso");
    sh(&format!("genisoimage -udf -input-charset utf-8 -iso-level 3 -o {} {} >/dev/null 2>&1", iso.display(), t.display()));
    Some(iso)
}

#[test]
fn reads_udf_listing_and_files_and_the_wim_inside() {
    let dir = tempfile::tempdir().unwrap();
    let Some(iso) = build(dir.path()) else { return };
    let tree = dir.path().join("tree");
    let mut v = UdfVolume::open(fs::File::open(&iso).unwrap()).expect("open UDF");

    // Listing, case-insensitivity, nesting, unicode, many entries, empty files.
    let mut top: Vec<_> = v.read_dir(&v.root()).unwrap().into_iter().map(|e| e.name).collect();
    top.sort();
    assert_eq!(top, ["boot", "bootmgr", "efi", "sources"]);
    assert_eq!(v.find("SOURCES/Setup.EXE").unwrap().unwrap().size, 100_000);
    assert_eq!(v.find("efi/microsoft/boot/bcd").unwrap().unwrap().size, 20_000);
    assert_eq!(v.find("sources/empty.txt").unwrap().unwrap().size, 0);
    assert!(v.find("sources/h\u{e9}llo w\u{f6}rld.txt").unwrap().is_some());
    assert!(v.find("sources/nope").unwrap().is_none());
    let lang = v.find("sources/lang").unwrap().unwrap();
    assert_eq!(v.read_dir(&lang).unwrap().len(), 200);

    // Byte-exact content, whole and in random pieces.
    for p in ["bootmgr", "boot/boot.sdi", "sources/boot.wim", "sources/install.wim", "efi/boot/bootx64.efi"] {
        let want = fs::read(tree.join(p)).unwrap();
        let e = v.find(p).unwrap().unwrap_or_else(|| panic!("{p} missing"));
        assert_eq!(e.size as usize, want.len(), "{p} size");
        assert!(v.read_file(&e, u64::MAX).unwrap() == want, "{p} content");
        let mut x = 7u64;
        for _ in 0..50 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let off = (x >> 33) as usize % (want.len() - 1);
            let n = 1 + (x >> 20) as usize % (want.len() - off).min(50_000);
            let mut b = vec![0u8; n];
            v.read_file_at(&e, off as u64, &mut b).unwrap();
            assert!(b == want[off..off + n], "{p}: {off}+{n}");
        }
    }

    // WIM: the header and XML of boot.wim, read straight out of the UDF file.
    let e = v.find("sources/boot.wim").unwrap().unwrap();
    let len = e.size;
    let info = wim::read_info(&mut UdfFileReader { vol: &mut v, entry: e }, len).expect("WIM header");
    assert_eq!(info.image_count, 2);
    assert_eq!(info.compression(), "LZX");
    assert_eq!(info.total_parts, 1);
    let names: Vec<_> = info.images.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["Microsoft Windows PE (x64)", "Microsoft Windows Setup (x64)"]);
    let e = v.find("sources/install.wim").unwrap().unwrap();
    let len = e.size;
    let info = wim::read_info(&mut UdfFileReader { vol: &mut v, entry: e }, len).unwrap();
    assert_eq!(info.images.len(), 1);
    assert_eq!(info.images[0].name, "Windows 11 Pro");
}

#[test]
fn rejects_non_udf_and_non_wim() {
    assert!(UdfVolume::open(std::io::Cursor::new(vec![0u8; 600 * 2048])).is_err());
    let mut c = std::io::Cursor::new(vec![0u8; 1000]);
    assert!(wim::read_info(&mut c, 1000).is_none());
}
