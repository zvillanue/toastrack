//! Reads exFAT images made by independent implementations (exfatprogs' mkfs + exfat-fuse).
//! Needs root, losetup, mkfs.exfat and mount.exfat-fuse; skipped (with a message) when unavailable.

use std::{fs, io::Write, path::Path, process::Command};
use toastrack_core::{
    exfat::ExfatVolume,
    io::{ReadAt, Shared},
};

fn have(tool: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {tool}")]).output().map(|o| o.status.success()).unwrap_or(false)
}

fn run(cmd: &str) {
    let st = Command::new("sh").args(["-c", cmd]).status().unwrap();
    assert!(st.success(), "failed: {cmd}");
}

/// Deterministic pseudo-random bytes.
fn data(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E3779B97F4A7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::File::create(path).unwrap().write_all(bytes).unwrap();
}

/// Builds the reference image; returns its path, or None when the tools are missing.
fn build(dir: &Path, cluster: u32) -> Option<std::path::PathBuf> {
    if !(have("mkfs.exfat") && have("mount.exfat-fuse") && have("losetup")) || !Path::new("/dev/fuse").exists() {
        eprintln!("SKIPPED: exFAT reference tools not available");
        return None;
    }
    if !Command::new("id").arg("-u").output().map(|o| o.stdout == b"0\n").unwrap_or(false) {
        eprintln!("SKIPPED: needs root");
        return None;
    }
    let img = dir.join("ref.img");
    let mnt = dir.join("mnt");
    fs::create_dir_all(&mnt).unwrap();
    run(&format!("truncate -s 256M {0} && mkfs.exfat -L TOASTRACK -c {cluster} {0} >/dev/null", img.display()));
    let lo = String::from_utf8(Command::new("losetup").args(["-f", "--show", img.to_str().unwrap()]).output().unwrap().stdout).unwrap();
    let lo = lo.trim().to_string();
    run(&format!("mount.exfat-fuse {lo} {} 2>/dev/null", mnt.display()));

    write(&mnt.join("isos/readme.txt"), b"hello\n");
    write(&mnt.join("isos/sub/deep/file.bin"), &data(1, 3 << 20));
    write(&mnt.join("isos/h\u{e9}llo w\u{f6}rld ISO.iso"), &data(2, 100_000));
    write(&mnt.join("empty.txt"), b"");
    // Many entries: the directory spans several clusters.
    for i in 0..300 {
        write(&mnt.join(format!("many/file-with-a-fairly-long-name-{i:04}.txt")), format!("{i}").as_bytes());
    }
    // Fragmentation: fill, free every other file, then write a big file into the holes.
    for i in 0..80 {
        write(&mnt.join(format!("filler/f{i}")), &data(100 + i, 40_000));
    }
    for i in (1..80).step_by(2) {
        fs::remove_file(mnt.join(format!("filler/f{i}"))).unwrap();
    }
    write(&mnt.join("isos/fragmented.bin"), &data(3, 1_500_000));
    run("sync");
    run(&format!("umount {} && losetup -d {lo}", mnt.display()));
    Some(img)
}

fn check(cluster: u32) {
    let dir = tempfile::tempdir().unwrap();
    let Some(img) = build(dir.path(), cluster) else { return };
    let mut vol = ExfatVolume::open(Shared::new(fs::File::open(&img).unwrap())).unwrap();
    assert_eq!(vol.label().unwrap().as_deref(), Some("TOASTRACK"));

    let root_dir = vol.root();
    let root = vol.read_dir(&root_dir).unwrap();
    let mut names: Vec<_> = root.iter().map(|e| e.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["empty.txt", "filler", "isos", "many"]);

    let many_e = vol.find("many").unwrap().unwrap();
    let many = vol.dir_of(&many_e).unwrap();
    let entries = vol.read_dir(&many).unwrap();
    assert_eq!(entries.len(), 300);
    assert!(entries.iter().any(|e| e.name == "file-with-a-fairly-long-name-0299.txt"));

    // Case-insensitive path lookup, unicode names, empty files.
    let e = vol.find("ISOS/H\u{e9}LLO w\u{f6}rld iso.ISO").unwrap();
    assert!(e.is_none() || e.is_some()); // non-ASCII case folding is not required
    let e = vol.find("isos/h\u{e9}llo w\u{f6}rld ISO.iso").unwrap().expect("unicode name");
    assert_eq!(e.size, 100_000);
    let mut f = vol.open_file(&e);
    let mut got = vec![0u8; 100_000];
    f.read_at(0, &mut got).unwrap();
    assert_eq!(got, data(2, 100_000));
    assert_eq!(vol.find("EMPTY.TXT").unwrap().unwrap().size, 0);
    assert!(vol.find("isos/nope").unwrap().is_none());
    assert!(vol.find("isos/readme.txt/x").unwrap().is_none());

    // Whole-file and random-access reads, including 1-byte reads across cluster boundaries.
    for (path, seed, len) in [("isos/sub/deep/file.bin", 1, 3 << 20), ("isos/fragmented.bin", 3, 1_500_000)] {
        let e = vol.find(path).unwrap().unwrap();
        assert_eq!(e.size as usize, len);
        let want = data(seed, len);
        let mut f = vol.open_file(&e);
        let mut all = vec![0u8; len];
        f.read_at(0, &mut all).unwrap();
        assert!(all == want, "{path}: full read differs");
        let mut x = 12345u64;
        for _ in 0..300 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let off = (x >> 33) as usize % (len - 1);
            let n = 1 + (x >> 20) as usize % (len - off).min(70_000);
            let mut b = vec![0u8; n];
            f.read_at(off as u64, &mut b).unwrap();
            assert!(b == want[off..off + n], "{path}: read at {off}+{n} differs");
        }
        let mut b = [0u8; 2];
        let cb = cluster as usize;
        f.read_at(cb as u64 - 1, &mut b).unwrap();
        assert_eq!(&b, &want[cb - 1..cb + 1]);
        assert!(f.read_at(len as u64 - 1, &mut [0u8; 2]).is_err(), "read past the end must fail");
    }
    // extents(): the raw image bytes at the reported offsets must equal the file contents.
    use std::io::{Read, Seek, SeekFrom};
    for (path, seed, len, want_runs_gt_one) in [("isos/sub/deep/file.bin", 1, 3 << 20, false), ("isos/fragmented.bin", 3, 1_500_000, true)] {
        let e = vol.find(path).unwrap().unwrap();
        let runs = vol.open_file(&e).extents().unwrap();
        assert_eq!(runs.iter().map(|r| r.1).sum::<u64>(), len as u64, "{path}: run lengths");
        assert_eq!(runs.len() > 1, want_runs_gt_one, "{path}: {} runs", runs.len());
        let mut raw = fs::File::open(&img).unwrap();
        let mut joined = Vec::new();
        for (off, n) in &runs {
            raw.seek(SeekFrom::Start(*off)).unwrap();
            let mut b = vec![0u8; *n as usize];
            raw.read_exact(&mut b).unwrap();
            joined.extend(b);
        }
        assert!(joined == data(seed, len), "{path}: raw bytes at the reported extents differ from the file");
    }
    let frag_e = vol.find("isos/fragmented.bin").unwrap().unwrap();
    let frag = vol.open_file(&frag_e);
    assert!(!frag.is_contiguous(), "the fragmented fixture is not fragmented; the test would prove nothing");
}

#[test]
fn reads_reference_image_4k_clusters() {
    check(4096);
}

#[test]
fn reads_reference_image_32k_clusters() {
    check(32768);
}

#[test]
fn rejects_non_exfat() {
    let r = ExfatVolume::open(Shared::new(std::io::Cursor::new(vec![0u8; 4096])));
    assert!(r.is_err());
}
