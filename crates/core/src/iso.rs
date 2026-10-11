//! Read-only ISO 9660 walker (enough to list files and extract one).

use std::io::{self, Read, Seek, SeekFrom};

const SECTOR: u64 = 2048;

#[derive(Debug, thiserror::Error)]
pub enum IsoError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("not an ISO 9660 image")]
    NotIso,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub lba: u32,
    pub size: u32,
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
    Some((Entry { name, is_dir, lba, size }, len))
}

impl<R: Read + Seek> Iso<R> {
    pub fn open(mut r: R) -> Result<Self, IsoError> {
        let mut sector = [0u8; SECTOR as usize];
        r.seek(SeekFrom::Start(16 * SECTOR))?;
        r.read_exact(&mut sector).map_err(|_| IsoError::NotIso)?;
        if &sector[1..6] != b"CD001" {
            return Err(IsoError::NotIso);
        }
        let (root, _) = parse_record(&sector[156..190]).ok_or(IsoError::NotIso)?;
        Ok(Self { r, root })
    }

    pub fn read_dir(&mut self, dir: &Entry) -> Result<Vec<Entry>, IsoError> {
        let mut buf = vec![0u8; dir.size as usize];
        self.r.seek(SeekFrom::Start(dir.lba as u64 * SECTOR))?;
        self.r.read_exact(&mut buf)?;
        let (mut out, mut pos) = (Vec::new(), 0usize);
        while pos < buf.len() {
            if buf[pos] == 0 {
                pos = (pos / SECTOR as usize + 1) * SECTOR as usize; // records never span sectors
                continue;
            }
            match parse_record(&buf[pos..]) {
                Some((e, len)) => {
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
        self.r.seek(SeekFrom::Start(e.lba as u64 * SECTOR))?;
        self.r.read_exact(&mut buf)?;
        Ok(buf)
    }
}
