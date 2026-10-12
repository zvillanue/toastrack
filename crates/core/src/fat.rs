//! Read-only FAT12/16/32, enough to look inside an ISO's El Torito EFI boot image.

use crate::io::{IoError, ReadAt};
use alloc::{string::String, vec, vec::Vec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatError {
    Io,
    NotFat,
    Corrupt,
}

impl From<IoError> for FatError {
    fn from(_: IoError) -> Self {
        FatError::Io
    }
}

type Result<T> = core::result::Result<T, FatError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FatEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u32,
    first_cluster: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Fat12,
    Fat16,
    Fat32,
}

pub struct FatImage<R> {
    src: R,
    kind: Kind,
    bps: u64,
    spc: u64,
    fat_off: u64,
    root_off: u64, // FAT12/16: fixed root directory
    root_len: u64,
    root_cluster: u32, // FAT32
    data_off: u64,
    clusters: u32,
}

fn u16le(b: &[u8], o: usize) -> u64 {
    u16::from_le_bytes([b[o], b[o + 1]]) as u64
}
fn u32le(b: &[u8], o: usize) -> u64 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as u64
}

const MAX_READ: u64 = 64 << 20;

impl<R: ReadAt> FatImage<R> {
    pub fn open(mut src: R) -> Result<Self> {
        let mut b = [0u8; 512];
        src.read_at(0, &mut b).map_err(|_| FatError::NotFat)?;
        if b[510] != 0x55 || b[511] != 0xAA {
            return Err(FatError::NotFat);
        }
        let (bps, spc, reserved, nfats) = (u16le(&b, 11), b[13] as u64, u16le(&b, 14), b[16] as u64);
        if !matches!(bps, 512 | 1024 | 2048 | 4096) || spc == 0 || !spc.is_power_of_two() || nfats == 0 {
            return Err(FatError::NotFat);
        }
        let root_entries = u16le(&b, 17);
        let total = if u16le(&b, 19) != 0 { u16le(&b, 19) } else { u32le(&b, 32) };
        let fat_size = if u16le(&b, 22) != 0 { u16le(&b, 22) } else { u32le(&b, 36) };
        let root_sectors = (root_entries * 32).div_ceil(bps);
        let data_start = reserved + nfats * fat_size + root_sectors;
        if total <= data_start || fat_size == 0 {
            return Err(FatError::NotFat);
        }
        let clusters = ((total - data_start) / spc) as u32;
        let kind = if clusters < 4085 { Kind::Fat12 } else if clusters < 65525 { Kind::Fat16 } else { Kind::Fat32 };
        Ok(Self {
            src,
            kind,
            bps,
            spc,
            fat_off: reserved * bps,
            root_off: (reserved + nfats * fat_size) * bps,
            root_len: root_sectors * bps,
            root_cluster: u32le(&b, 44) as u32,
            data_off: data_start * bps,
            clusters,
        })
    }

    fn cluster_bytes(&self) -> u64 {
        self.bps * self.spc
    }

    fn next(&mut self, c: u32) -> Result<Option<u32>> {
        let (raw, eoc) = match self.kind {
            Kind::Fat12 => {
                let pos = self.fat_off + c as u64 * 3 / 2;
                let mut b = [0u8; 2];
                self.src.read_at(pos, &mut b)?;
                let v = u16::from_le_bytes(b) as u32;
                (if c % 2 == 0 { v & 0xFFF } else { v >> 4 }, 0xFF8)
            }
            Kind::Fat16 => {
                let mut b = [0u8; 2];
                self.src.read_at(self.fat_off + c as u64 * 2, &mut b)?;
                (u16::from_le_bytes(b) as u32, 0xFFF8)
            }
            Kind::Fat32 => {
                let mut b = [0u8; 4];
                self.src.read_at(self.fat_off + c as u64 * 4, &mut b)?;
                (u32::from_le_bytes(b) & 0x0FFF_FFFF, 0x0FFF_FFF8)
            }
        };
        if raw >= eoc {
            Ok(None)
        } else if raw < 2 || raw >= self.clusters + 2 {
            Err(FatError::Corrupt)
        } else {
            Ok(Some(raw))
        }
    }

    /// All the bytes of a cluster chain (at most `limit`).
    fn read_chain(&mut self, first: u32, limit: u64) -> Result<Vec<u8>> {
        let cb = self.cluster_bytes();
        let mut out = Vec::new();
        let mut c = Some(first);
        let mut hops = 0u32;
        while let Some(cl) = c {
            if out.len() as u64 >= limit || hops > self.clusters {
                break;
            }
            let at = out.len();
            out.resize(at + cb as usize, 0);
            self.src.read_at(self.data_off + (cl as u64 - 2) * cb, &mut out[at..])?;
            c = self.next(cl)?;
            hops += 1;
        }
        out.truncate(limit as usize);
        Ok(out)
    }

    fn dir_bytes(&mut self, first: Option<u32>) -> Result<Vec<u8>> {
        match first {
            Some(c) if c >= 2 => self.read_chain(c, MAX_READ),
            _ if self.kind == Kind::Fat32 => self.read_chain(self.root_cluster, MAX_READ),
            _ => {
                let mut v = vec![0u8; self.root_len as usize];
                self.src.read_at(self.root_off, &mut v)?;
                Ok(v)
            }
        }
    }

    fn list(&mut self, dir: Option<u32>) -> Result<Vec<FatEntry>> {
        let d = self.dir_bytes(dir)?;
        let (mut out, mut lfn): (Vec<FatEntry>, Vec<(u8, [u16; 13])>) = (Vec::new(), Vec::new());
        for e in d.chunks_exact(32) {
            match e[0] {
                0x00 => break,
                0xE5 => {
                    lfn.clear();
                    continue;
                }
                _ => {}
            }
            if e[11] == 0x0F {
                let mut u = [0u16; 13];
                for (i, o) in [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30].iter().enumerate() {
                    u[i] = u16::from_le_bytes([e[*o], e[*o + 1]]);
                }
                lfn.push((e[0] & 0x1F, u));
                continue;
            }
            if e[11] & 0x08 != 0 {
                lfn.clear(); // volume label
                continue;
            }
            let name = if lfn.is_empty() {
                let base: String = String::from_utf8_lossy(&e[0..8]).trim_end().into();
                let ext: String = String::from_utf8_lossy(&e[8..11]).trim_end().into();
                if ext.is_empty() { base } else { alloc::format!("{base}.{ext}") }
            } else {
                lfn.sort_by_key(|(n, _)| *n);
                let units: Vec<u16> = lfn.iter().flat_map(|(_, u)| u.iter().copied()).take_while(|&c| c != 0 && c != 0xFFFF).collect();
                char::decode_utf16(units).map(|c| c.unwrap_or('?')).collect()
            };
            lfn.clear();
            let first = (u16le(e, 20) << 16 | u16le(e, 26)) as u32;
            out.push(FatEntry { name, is_dir: e[11] & 0x10 != 0, size: u32le(e, 28) as u32, first_cluster: first });
        }
        Ok(out)
    }

    /// Entries of the directory at `path` (`""` = root).
    pub fn list_dir(&mut self, path: &str) -> Result<Vec<FatEntry>> {
        if path.trim_matches('/').is_empty() {
            return self.list(None);
        }
        match self.find(path)? {
            Some(e) if e.is_dir => self.list(Some(e.first_cluster)),
            _ => Ok(Vec::new()),
        }
    }

    /// Case-insensitive lookup of a `/`-separated path.
    pub fn find(&mut self, path: &str) -> Result<Option<FatEntry>> {
        let mut dir = None;
        let mut parts = path.split('/').filter(|p| !p.is_empty()).peekable();
        while let Some(part) = parts.next() {
            let Some(e) = self.list(dir)?.into_iter().find(|e| e.name.eq_ignore_ascii_case(part)) else { return Ok(None) };
            if parts.peek().is_none() {
                return Ok(Some(e));
            }
            if !e.is_dir {
                return Ok(None);
            }
            dir = Some(e.first_cluster);
        }
        Ok(None)
    }

    pub fn read_file(&mut self, e: &FatEntry) -> Result<Vec<u8>> {
        if e.is_dir || e.size == 0 {
            return Ok(Vec::new());
        }
        if e.size as u64 > MAX_READ {
            return Err(FatError::Corrupt);
        }
        let v = self.read_chain(e.first_cluster, e.size as u64)?;
        if (v.len() as u32) < e.size {
            return Err(FatError::Corrupt);
        }
        Ok(v)
    }
}
