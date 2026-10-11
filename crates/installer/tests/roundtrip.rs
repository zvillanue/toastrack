use std::{fs::OpenOptions, io::{Read, Seek, SeekFrom}, path::PathBuf};
use toastrack_installer::{
    bundle::Bundle,
    install::{install, install_unchecked, read_parts, update_unchecked as update, verify},
    layout::{self, MIB},
};

fn synthetic(tag: u8) -> Bundle {
    // Not valid PE files: only usable with install_unchecked, which skips signature checks.
    Bundle { shim: vec![tag; 50_000], mok_manager: vec![tag + 1; 30_000], loader: vec![tag + 2; 70_000], cert: vec![tag + 3; 800] }
}

fn image(size_mib: u64) -> (tempfile::TempDir, PathBuf, std::fs::File) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stick.img");
    let f = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
    f.set_len(size_mib * MIB).unwrap();
    (dir, path, f)
}

#[test]
fn install_then_verify_then_update() {
    let (_d, _p, f) = image(400);
    let (f, parts) = install_unchecked(f, &synthetic(1)).unwrap();
    assert_eq!(parts.esp.0, MIB);
    assert_eq!(parts.esp.1, layout::ESP_BYTES);
    assert_eq!(parts.data.0, MIB + layout::ESP_BYTES);

    let (f, problems) = verify(f, &synthetic(1)).unwrap();
    assert!(problems.is_empty(), "{problems:?}");

    // A different bundle must be reported as differing.
    let (f, problems) = verify(f, &synthetic(9)).unwrap();
    assert_eq!(problems.len(), 4, "{problems:?}");

    // Update replaces the boot files and leaves the data partition alone.
    let f = {
        let (mut f, parts) = read_parts(f).unwrap();
        f.seek(SeekFrom::Start(parts.data.0 + 5 * MIB)).unwrap();
        use std::io::Write;
        f.write_all(b"MARKER-IN-DATA-PARTITION").unwrap();
        f
    };
    let f = update(f, &synthetic(9)).unwrap();
    let (mut f, problems) = verify(f, &synthetic(9)).unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    let (f2, parts) = read_parts(&mut f).unwrap();
    f2.seek(SeekFrom::Start(parts.data.0 + 5 * MIB)).unwrap();
    let mut buf = [0u8; 24];
    f2.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"MARKER-IN-DATA-PARTITION");
}

#[test]
fn volume_ids_differ_between_installs() {
    let id = |f: std::fs::File| {
        let (mut f, parts) = read_parts(f).unwrap();
        f.seek(SeekFrom::Start(parts.data.0 + 67)).unwrap(); // FAT32 volume ID at offset 67 of the boot sector
        let mut b = [0u8; 4];
        f.read_exact(&mut b).unwrap();
        b
    };
    let (_d1, _p1, f1) = image(400);
    let (_d2, _p2, f2) = image(400);
    let (f1, _) = install_unchecked(f1, &synthetic(1)).unwrap();
    let (f2, _) = install_unchecked(f2, &synthetic(1)).unwrap();
    assert_ne!(id(f1), id(f2));
}

#[test]
fn rejects_too_small_disk() {
    let (_d, _p, f) = image(100);
    assert!(install_unchecked(f, &synthetic(1)).is_err());
}

#[test]
fn install_refuses_unvalidated_bundle() {
    let (_d, _p, f) = image(400);
    let err = install(f, &synthetic(1)).unwrap_err().to_string();
    assert!(err.contains("not a valid PE") || err.contains("bad bundle"), "{err}");
}

/// Uses a real bundle (from scripts/make-dev-bundle.sh) when TOASTRACK_TEST_BUNDLE is set.
#[test]
fn real_bundle_installs_and_verifies() {
    let Ok(dir) = std::env::var("TOASTRACK_TEST_BUNDLE") else { return };
    let bundle = Bundle::load(std::path::Path::new(&dir)).unwrap();
    let (_d, _p, f) = image(400);
    let (f, _) = install(f, &bundle).unwrap();
    let (_, problems) = verify(f, &bundle).unwrap();
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn bundle_validation_explains_the_common_mistakes() {
    let Ok(dir) = std::env::var("TOASTRACK_TEST_BUNDLE") else { return };
    let good = Bundle::load(std::path::Path::new(&dir)).unwrap();

    // Loader signed with a different key than the certificate we would ask users to enroll.
    let mut wrong_cert = good.clone();
    wrong_cert.cert = std::fs::read(format!("{dir}/other.cer")).unwrap_or_default();
    if !wrong_cert.cert.is_empty() {
        assert!(wrong_cert.validate().unwrap_err().to_string().contains("not signed with the certificate"));
    }

    // Shim that Microsoft did not sign (use the loader in its place).
    let mut bad_shim = good.clone();
    bad_shim.shim = good.loader.clone();
    assert!(bad_shim.validate().unwrap_err().to_string().contains("not signed by Microsoft"));

    // Unsigned loader.
    let unsigned = std::fs::read(format!("{dir}/unsigned.efi")).unwrap_or_default();
    if !unsigned.is_empty() {
        let mut b = good.clone();
        b.loader = unsigned;
        assert!(b.validate().unwrap_err().to_string().contains("unsigned"));
    }
}
