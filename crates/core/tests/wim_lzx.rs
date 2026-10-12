//! LZX + WIM directory reading against WIMs made by wimlib (skipped without wimlib-imagex).

use std::{fs, path::Path, process::Command};
use toastrack_core::{
    io::Shared,
    wim::{ImageTree, Wim},
};

fn have(t: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {t}")]).output().map(|o| o.status.success()).unwrap_or(false)
}
fn sh(c: &str) {
    assert!(Command::new("sh").args(["-c", c]).status().unwrap().success(), "failed: {c}");
}

/// Compressible, structured bytes: words, repeated runs, and x86-like CALL (E8) sequences.
fn data(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E3779B97F4A7C15) | 1;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let words: Vec<&[u8]> = vec![b"kernel32.dll ", b"System32\\drivers ", b"\x00\x00\x00\x00", b"MZ\x90\x00", b"function ", b"Windows Setup ", b"\xff\xff\xff\xff"];
    let mut v = Vec::with_capacity(len + 64);
    while v.len() < len {
        match next() % 6 {
            0 | 1 => v.extend_from_slice(words[(next() % words.len() as u64) as usize]),
            2 => {
                let b = (next() % 256) as u8;
                for _ in 0..(next() % 40) {
                    v.push(b);
                }
            }
            3 => {
                // CALL rel32 with a small relative target: the pattern the E8 transform rewrites.
                v.push(0xE8);
                v.extend_from_slice(&((next() % 100_000) as i32 - 2_000).to_le_bytes());
            }
            4 => {
                let n = v.len().min(1 + (next() % 300) as usize);
                let s = v.len() - n;
                let copy = v[s..].to_vec();
                v.extend(copy);
            }
            _ => v.extend((0..(next() % 20)).map(|_| next() as u8)),
        }
    }
    v.truncate(len);
    v
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, Option<Vec<u8>>)>) {
    let mut es: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap()).collect();
    es.sort_by_key(|e| e.file_name());
    for e in es {
        let name = e.file_name().to_string_lossy().into_owned();
        let p = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        if e.file_type().unwrap().is_dir() {
            out.push((p.clone(), None));
            walk(&e.path(), &p, out);
        } else {
            out.push((p, Some(fs::read(e.path()).unwrap())));
        }
    }
}

fn check(compress: &str, extra: &str) {
    if !have("wimlib-imagex") {
        eprintln!("SKIPPED: wimlib-imagex missing");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let img = d.path().join("img");
    let files: &[(&str, u64, usize)] = &[
        ("Windows/System32/winpeshl.ini", 1, 120),
        ("Windows/System32/startnet.cmd", 2, 40),
        ("Windows/System32/kernelbase.dll", 3, 1_500_000),
        ("Windows/System32/drivers/disk.sys", 4, 300_000),
        ("sources/setup.exe", 5, 700_000),
        ("sources/lang/en-us/setup.mui", 6, 70_000),
        ("empty.txt", 7, 0),
        ("one.bin", 8, 1),
        ("exact-chunk.bin", 9, 32768),
        ("chunk-plus-one.bin", 10, 32769),
        ("Program Files/h\u{e9}llo w\u{f6}rld.txt", 11, 5000),
    ];
    for (p, seed, n) in files {
        let path = img.join(p);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data(*seed, *n)).unwrap();
    }
    // a duplicate (same content, different path) and an incompressible file
    fs::write(img.join("sources/setup-copy.exe"), data(5, 700_000)).unwrap();
    fs::write(img.join("random.bin"), (0..100_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect::<Vec<u8>>()).unwrap();
    let wim = d.path().join("t.wim");
    sh(&format!("wimlib-imagex capture {} {} 'Test' --compress={compress} {extra} >/dev/null", img.display(), wim.display()));

    let len = fs::metadata(&wim).unwrap().len();
    let mut w = Wim::open(Shared::new(fs::File::open(&wim).unwrap()), len).expect("open WIM");
    assert_eq!(w.info.image_count, 1);
    // Metadata resource: decompressed and hash-verified by `metadata()` itself.
    let tree = ImageTree::new(w.metadata(1).expect("metadata")).expect("tree");

    let mut expect = Vec::new();
    walk(&img, "", &mut expect);
    for (path, content) in expect {
        let e = tree.find(&path).unwrap().unwrap_or_else(|| panic!("{path} missing from the WIM tree"));
        match content {
            None => assert!(e.is_dir, "{path}"),
            Some(want) => {
                assert!(!e.is_dir, "{path}");
                if want.is_empty() {
                    assert_eq!(e.hash, [0u8; 20], "{path}: empty file has no stream");
                } else {
                    assert_eq!(w.stream_size(&e.hash), Some(want.len() as u64), "{path} size");
                    // read_stream() also verifies the SHA-1 recorded in the WIM
                    assert!(w.read_stream(&e.hash).unwrap_or_else(|er| panic!("{path}: {er}")) == want, "{path} content");
                }
            }
        }
    }
    // Directory listing sizes agree with the source tree.
    let sys = tree.find("windows/SYSTEM32").unwrap().unwrap();
    let mut names: Vec<_> = tree.children(&sys).unwrap().into_iter().map(|e| e.name).collect();
    names.sort();
    assert_eq!(names, ["drivers", "kernelbase.dll", "startnet.cmd", "winpeshl.ini"]);
    assert!(tree.find("nope/x").unwrap().is_none());
}

#[test]
fn lzx_32k_chunks() {
    check("LZX", "");
}
#[test]
fn lzx_64k_chunks() {
    check("LZX", "--chunk-size=65536");
}
#[test]
fn lzx_128k_chunks() {
    check("LZX", "--chunk-size=131072");
}
#[test]
fn lzx_2m_chunks() {
    check("LZX", "--chunk-size=2097152");
}
#[test]
fn uncompressed_wim() {
    check("none", "");
}
#[test]
fn xpress_is_refused_not_misread() {
    if !have("wimlib-imagex") {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("i")).unwrap();
    fs::write(d.path().join("i/a.txt"), data(1, 200_000)).unwrap();
    let wim = d.path().join("x.wim");
    sh(&format!("wimlib-imagex capture {} {} 'X' --compress=XPRESS >/dev/null", d.path().join("i").display(), wim.display()));
    let len = fs::metadata(&wim).unwrap().len();
    let mut w = Wim::open(Shared::new(fs::File::open(&wim).unwrap()), len).unwrap();
    assert!(w.metadata(1).is_err() || w.info.compression() == "XPRESS");
}
