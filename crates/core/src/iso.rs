//! Read-only ISO 9660 walker (enough to list files and extract one).

use crate::io::{IoError, ReadAt};
use alloc::{string::{String, ToString}, vec, vec::Vec};

const SECTOR: u64 = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsoError {
    Io,
    NotIso,
}

impl From<IoError> for IsoError {
    fn from(_: IoError) -> Self {
        IsoError::Io
    }
}

impl core::fmt::Display for IsoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            IsoError::Io => "read error",
            IsoError::NotIso => "not an ISO 9660 image",
        })
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub lba: u32,
    pub size: u32,
    /// Absolute byte offset of this entry's directory record in the image (0 for the root), so a caller
    /// serving a modified view of the ISO can patch its size field.
    pub record_offset: u64,
}

impl Entry {
    /// Bytes available to this file without touching other data: its extent rounded up to whole sectors.
    pub fn capacity(&self) -> u64 {
        (self.size as u64).div_ceil(SECTOR).max(1) * SECTOR
    }

    /// Patches that turn this entry's recorded size into `new_size` (both-endian field in its directory record).
    pub fn size_patch(&self, new_size: u32) -> (u64, [u8; 8]) {
        let mut b = [0u8; 8];
        b[..4].copy_from_slice(&new_size.to_le_bytes());
        b[4..].copy_from_slice(&new_size.to_be_bytes());
        (self.record_offset + 10, b)
    }
}

pub struct Iso<R> {
    r: R,
    root: Entry,
}

fn parse_record(b: &[u8]) -> Option<(Entry, usize)> {
    let len = *b.first()? as usize;
    if len == 0 || b.len() < len || len < 34 {
        return None;
    }
    let lba = u32::from_le_bytes(b[2..6].try_into().ok()?);
    let size = u32::from_le_bytes(b[10..14].try_into().ok()?);
    let is_dir = b[25] & 2 != 0;
    let nlen = b[32] as usize;
    let raw = b.get(33..33 + nlen)?;
    let name = match raw {
        [0] => ".".to_string(),
        [1] => "..".to_string(),
        _ => {
            let s = String::from_utf8_lossy(raw).into_owned();
            // strip ";1" version suffix and trailing dot of plain ISO 9660 names
            let s = s.split(';').next().unwrap_or("").trim_end_matches('.').to_string();
            s
        }
    };
    Some((Entry { name, is_dir, lba, size, record_offset: 0 }, len))
}

impl<R: ReadAt> Iso<R> {
    pub fn open(mut r: R) -> Result<Self, IsoError> {
        let mut sector = [0u8; SECTOR as usize];
        r.read_at(16 * SECTOR, &mut sector).map_err(|_| IsoError::NotIso)?;
        if &sector[1..6] != b"CD001" {
            return Err(IsoError::NotIso);
        }
        let (root, _) = parse_record(&sector[156..190]).ok_or(IsoError::NotIso)?;
        Ok(Self { r, root })
    }

    pub fn read_dir(&mut self, dir: &Entry) -> Result<Vec<Entry>, IsoError> {
        let mut buf = vec![0u8; dir.size as usize];
        self.r.read_at(dir.lba as u64 * SECTOR, &mut buf)?;
        let (mut out, mut pos) = (Vec::new(), 0usize);
        while pos < buf.len() {
            if buf[pos] == 0 {
                pos = (pos / SECTOR as usize + 1) * SECTOR as usize; // records never span sectors
                continue;
            }
            match parse_record(&buf[pos..]) {
                Some((mut e, len)) => {
                    e.record_offset = dir.lba as u64 * SECTOR + pos as u64;
                    pos += len;
                    if e.name != "." && e.name != ".." {
                        out.push(e);
                    }
                }
                None => break,
            }
        }
        Ok(out)
    }

    /// Case-insensitive lookup of a `/`-separated path.
    pub fn find(&mut self, path: &str) -> Result<Option<Entry>, IsoError> {
        let mut cur = self.root.clone();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            if !cur.is_dir {
                return Ok(None);
            }
            match self.read_dir(&cur)?.into_iter().find(|e| e.name.eq_ignore_ascii_case(part)) {
                Some(e) => cur = e,
                None => return Ok(None),
            }
        }
        Ok(Some(cur))
    }

    pub fn read_file(&mut self, e: &Entry) -> Result<Vec<u8>, IsoError> {
        let mut buf = vec![0u8; e.size as usize];
        self.r.read_at(e.lba as u64 * SECTOR, &mut buf)?;
        Ok(buf)
    }
}
