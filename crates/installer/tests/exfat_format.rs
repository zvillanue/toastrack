use std::{fs::OpenOptions, process::Command};
use toastrack_core::{exfat::ExfatVolume, io::{ReadAt, Shared}};
use toastrack_installer::exfat_format::{format, Node};

fn have(tool: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {tool}")]).output().map(|o| o.status.success()).unwrap_or(false)
}

fn make(size: u64) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.img");
    let mut f = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
    f.set_len(size).unwrap();
    let readme = b"Copy ISO files here.\r\n".to_vec();
    let big: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let tree = vec![
        Node::Dir(
            "isos",
            vec![
                Node::File("README.txt", &readme),
                Node::Dir("sub folder", vec![Node::File("a-rather-long-file-name-that-needs-several-name-entries.bin", &big)]),
                Node::File("h\u{e9}llo.txt", b"unicode"),
                Node::File("empty", b""),
            ],
        ),
        Node::File("top.bin", &big),
    ];
    format(&mut f, size, 1 << 20, "TOASTRACK", 0xCAFE_F00D, &tree).unwrap();
    (dir, path)
}

#[test]
fn independent_fsck_accepts_the_output() {
    if !have("fsck.exfat") {
        eprintln!("SKIPPED: fsck.exfat not installed");
        return;
    }
    for size in [300u64 << 20, 1 << 30, 3 << 30] {
        let (_d, path) = make(size);
        let out = Command::new("fsck.exfat").arg(&path).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success() && text.contains("clean"), "size {size}: fsck.exfat said:\n{text}");
        let dump = Command::new("dump.exfat").arg(&path).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
        assert!(dump.contains("TOASTRACK"), "{dump}");
        assert!(dump.contains("0xcafef00d"), "{dump}");
    }
}

#[test]
fn our_reader_reads_what_the_formatter_wrote() {
    let (_d, path) = make(300 << 20);
    let mut vol = ExfatVolume::open(Shared::new(std::fs::File::open(&path).unwrap())).unwrap();
    assert_eq!(vol.label().unwrap().as_deref(), Some("TOASTRACK"));
    assert_eq!(vol.serial(), 0xCAFE_F00D);
    let big: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
    for p in ["top.bin", "ISOS/sub folder/a-rather-long-file-name-that-needs-several-name-entries.bin"] {
        let e = vol.find(p).unwrap().unwrap_or_else(|| panic!("{p} missing"));
        let mut got = vec![0u8; e.size as usize];
        vol.open_file(&e).read_at(0, &mut got).unwrap();
        assert!(got == big, "{p}");
    }
    let e = vol.find("isos/README.txt").unwrap().unwrap();
    let mut got = vec![0u8; e.size as usize];
    vol.open_file(&e).read_at(0, &mut got).unwrap();
    assert_eq!(got, b"Copy ISO files here.\r\n");
    assert_eq!(vol.find("isos/empty").unwrap().unwrap().size, 0);
    assert!(vol.find("isos/h\u{e9}llo.txt").unwrap().is_some());
}

#[test]
fn rejects_unreasonable_input() {
    let mut c = std::io::Cursor::new(vec![0u8; 1 << 20]);
    assert!(format(&mut c, 1 << 20, 0, "X", 1, &[]).is_err()); // too small
    let (_d, path) = make(300 << 20);
    drop(path);
    let mut f = std::io::Cursor::new(Vec::new());
    assert!(format(&mut f, 300 << 20, 0, "THIS-LABEL-IS-TOO-LONG", 1, &[]).is_err());
}

/// The Windows helper layout (answer file at the root, a folder with a 65 KB executable) must satisfy fsck.exfat too.
#[test]
fn fsck_accepts_the_windows_helper_layout() {
    if !have("fsck.exfat") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("w.img");
    let mut f = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
    f.set_len(1 << 30).unwrap();
    let exe: Vec<u8> = (0..65_450u32).map(|i| (i * 31 % 251) as u8).collect();
    let xml = vec![b'x'; 1500];
    let tree = vec![
        Node::Dir("isos", vec![Node::File("README.txt", b"hi")]),
        Node::File("autounattend.xml", &xml),
        Node::Dir("toastrack", vec![Node::File("attach.exe", &exe)]),
    ];
    format(&mut f, 1 << 30, 1 << 20, "TOASTRACK", 0x1234_5678, &tree).unwrap();
    let out = Command::new("fsck.exfat").arg(&path).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success() && text.contains("clean"), "{text}");
}
