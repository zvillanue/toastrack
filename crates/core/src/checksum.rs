//! ISO integrity: hash a file in chunks and compare it with the checksum published next to it.
//!
//! Distros publish SHA-256 sums either as one `SHA256SUMS` file for a folder or as a per-file `<name>.sha256`; both are
//! understood, in the GNU (`<hash> *<name>`) and BSD (`SHA256 (<name>) = <hash>`) layouts.

use crate::io::{IoError, ReadAt};
use alloc::{string::String, vec::Vec};
use sha2::{Digest, Sha256};

const CHUNK: usize = 1 << 20;

/// Names a checksum for `iso_name` may be kept in, in the order they are tried.
pub fn sidecar_names(iso_name: &str) -> Vec<String> {
    let mut v = Vec::new();
    for ext in ["sha256", "sha256sum"] {
        v.push(alloc::format!("{iso_name}.{ext}"));
    }
    for n in ["SHA256SUMS", "sha256sums", "SHA256SUM", "sha256sum.txt", "SHA256SUMS.txt"] {
        v.push(n.into());
    }
    v
}

/// SHA-256 of `size` bytes of `r`. `progress(done)` is called after every chunk; returning false stops early (`Ok(None)`).
pub fn sha256_of<R: ReadAt>(r: &mut R, size: u64, mut progress: impl FnMut(u64) -> bool) -> Result<Option<[u8; 32]>, IoError> {
    let mut h = Sha256::new();
    let mut buf = alloc::vec![0u8; CHUNK];
    let mut done = 0u64;
    while done < size {
        let n = (size - done).min(CHUNK as u64) as usize;
        r.read_at(done, &mut buf[..n])?;
        h.update(&buf[..n]);
        done += n as u64;
        if !progress(done) {
            return Ok(None);
        }
    }
    Ok(Some(h.finalize().into()))
}

pub fn to_hex(d: &[u8; 32]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    d.iter().flat_map(|b| [H[(b >> 4) as usize] as char, H[(b & 15) as usize] as char]).collect()
}

fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The expected SHA-256 (lower-case hex) for `iso_name` from the text of a checksum file, if it lists one.
/// A file holding just a bare hash (optionally followed by a name) counts for any name it does not contradict.
pub fn expected_in(text: &str, iso_name: &str) -> Option<String> {
    let mut bare: Option<String> = None;
    let mut listed = 0;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        // BSD style: SHA256 (name) = hash
        if let Some(rest) = line.strip_prefix("SHA256 (") {
            if let Some((name, hash)) = rest.split_once(") = ") {
                listed += 1;
                if is_hash(hash.trim()) && name.eq_ignore_ascii_case(iso_name) {
                    return Some(hash.trim().to_ascii_lowercase());
                }
            }
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let (hash, name) = (parts.next().unwrap_or(""), parts.next().map(|n| n.trim_start().trim_start_matches('*')));
        if !is_hash(hash) {
            continue;
        }
        match name {
            Some(n) => {
                listed += 1;
                // Sums made from another directory carry a path; only the file name matters.
                if n.eq_ignore_ascii_case(iso_name) || n.rsplit(['/', '\\']).next().is_some_and(|b| b.eq_ignore_ascii_case(iso_name)) {
                    return Some(hash.to_ascii_lowercase());
                }
            }
            None => bare = Some(hash.to_ascii_lowercase()),
        }
    }
    if listed == 0 { bare } else { None }
}

/// The file names a checksum file lists (for telling the user what it does contain).
pub fn listed_names(text: &str) -> Vec<String> {
    let mut v = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        if let Some(rest) = line.strip_prefix("SHA256 (") {
            if let Some((name, _)) = rest.split_once(") = ") {
                v.push(name.into());
            }
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        if parts.next().is_some_and(is_hash) {
            if let Some(n) = parts.next() {
                v.push(n.trim_start().trim_start_matches('*').into());
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    const H1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const H2: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    #[test]
    fn gnu_bsd_and_bare_layouts() {
        let gnu = alloc::format!("# comment\n{H1} *other.iso\n{H2}  ubuntu.iso\n");
        assert_eq!(expected_in(&gnu, "ubuntu.iso").as_deref(), Some(H2.to_ascii_lowercase().as_str()));
        assert_eq!(expected_in(&gnu, "missing.iso"), None);
        let bsd = alloc::format!("SHA256 (a.iso) = {H1}\nSHA256 (b.iso) = {H2}\n");
        assert_eq!(expected_in(&bsd, "a.iso").as_deref(), Some(H1));
        assert_eq!(expected_in(&alloc::format!("{H1}\n"), "x.iso").as_deref(), Some(H1));
        assert_eq!(expected_in(&alloc::format!("{H1}  x.iso\n"), "y.iso"), None);
        assert_eq!(expected_in(&alloc::format!("{H1}  ./dir/x.iso\n"), "x.iso").as_deref(), Some(H1));
        assert_eq!(expected_in("not a checksum file\n", "x.iso"), None);
        assert_eq!(expected_in(&alloc::format!("{H1} *Ubuntu.ISO\r\n"), "ubuntu.iso").as_deref(), Some(H1));
        assert_eq!(listed_names(&gnu), ["other.iso", "ubuntu.iso"]);
    }

    #[test]
    fn hashes_in_chunks_and_can_be_cancelled() {
        // SHA-256("abc")
        let data = b"abc".to_vec();
        let mut r = crate::io::Shared::new(std::io::Cursor::new(data));
        let d = sha256_of(&mut r, 3, |_| true).unwrap().unwrap();
        assert_eq!(to_hex(&d), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let big = alloc::vec![7u8; 3 * CHUNK + 5];
        let mut r2 = crate::io::Shared::new(std::io::Cursor::new(big));
        let mut calls = 0;
        assert!(sha256_of(&mut r2, 3 * CHUNK as u64 + 5, |_| { calls += 1; calls < 2 }).unwrap().is_none());
        assert_eq!(calls, 2);
    }
}
