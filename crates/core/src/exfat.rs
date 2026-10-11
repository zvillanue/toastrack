//! Read-only exFAT, for firmware that has no exFAT driver.
//!
//! Supports what a boot menu needs: the volume label, directory listing, case-insensitive path lookup
//! and random-access file reads (contiguous files and FAT chains, with run coalescing so large ISO
//! reads become few large I/Os).

use crate::io::{IoError, ReadAt};
use alloc::{string::String, vec, vec::Vec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExfatError {
    Io,
    NotExfat,
    Corrupt(&'static str),
}

impl From<IoError> for ExfatError {
    fn from(_: IoError) -> Self {
        ExfatError::Io
    }
}

impl core::fmt::Display for ExfatError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ExfatError::Io => f.write_str("read error"),
            ExfatError::NotExfat => f.write_str("not an exFAT volume"),
            ExfatError::Corrupt(w) => write!(f, "corrupt exFAT: {w}"),
        }
    }
}

type Result<T> = core::result::Result<T, ExfatError>;

const END_OF_CHAIN: u32 = 0xFFFF_FFF8; // >= this ends a chain
const CHECKPOINT_EVERY: u64 = 128;

#[derive(Debug, Clone, Copy)]
struct Geometry {
    bytes_per_sector: u64,
    cluster_bytes: u64,
    fat_offset: u64, // bytes
    heap_offset: u64, // bytes
    cluster_count: u32,
    root_cluster: u32,
    serial: u32,
}

impl Geometry {
    fn cluster_pos(&self, c: u32) -> u64 {
        self.heap_offset + (c as u64 - 2) * self.cluster_bytes
    }
    fn valid_cluster(&self, c: u32) -> bool {
        c >= 2 && c < self.cluster_count.saturating_add(2)
    }
}

/// Where a directory's (or file's) data lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    first: u32,
    /// Bytes of data; `None` for the root directory, whose size is only known from its FAT chain.
    len: Option<u64>,
    valid_len: Option<u64>,
    /// `NoFatChain`: clusters are consecutive and the FAT is not used.
    contiguous: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    extent: Extent,
}

pub struct ExfatVolume<R> {
    src: R,
    g: Geometry,
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

impl<R: ReadAt + Clone> ExfatVolume<R> {
    /// `src` must start at the first byte of the exFAT volume (the partition).
    pub fn open(mut src: R) -> Result<Self> {
        let mut b = [0u8; 512];
        src.read_at(0, &mut b).map_err(|_| ExfatError::NotExfat)?;
        if &b[3..11] != b"EXFAT   " || b[510] != 0x55 || b[511] != 0xAA {
            return Err(ExfatError::NotExfat);
        }
        let (bps_shift, spc_shift) = (b[108] as u32, b[109] as u32);
        if !(9..=12).contains(&bps_shift) || bps_shift + spc_shift > 25 {
            return Err(ExfatError::Corrupt("sector/cluster size"));
        }
        if b[110] != 1 {
            return Err(ExfatError::Corrupt("more than one FAT (TexFAT) is not supported"));
        }
        let bps = 1u64 << bps_shift;
        let g = Geometry {
            bytes_per_sector: bps,
            cluster_bytes: bps << spc_shift,
            fat_offset: u32le(&b, 80) as u64 * bps,
            heap_offset: u32le(&b, 88) as u64 * bps,
            cluster_count: u32le(&b, 92),
            root_cluster: u32le(&b, 96),
            serial: u32le(&b, 100),
        };
        if !g.valid_cluster(g.root_cluster) {
            return Err(ExfatError::Corrupt("root directory cluster"));
        }
        Ok(Self { src, g })
    }

    pub fn serial(&self) -> u32 {
        self.g.serial
    }

    pub fn root(&self) -> Dir {
        Dir(Extent { first: self.g.root_cluster, len: None, valid_len: None, contiguous: false })
    }

    pub fn dir_of(&self, e: &DirEntry) -> Option<Dir> {
        e.is_dir.then_some(Dir(e.extent))
    }

    fn fat_entry(&mut self, cache: &mut FatCache, c: u32) -> Result<u32> {
        let pos = self.g.fat_offset + c as u64 * 4;
        let sec = pos / self.g.bytes_per_sector;
        if cache.sector != Some(sec) {
            cache.buf.resize(self.g.bytes_per_sector as usize, 0);
            self.src.read_at(sec * self.g.bytes_per_sector, &mut cache.buf)?;
            cache.sector = Some(sec);
        }
        Ok(u32le(&cache.buf, (pos % self.g.bytes_per_sector) as usize))
    }

    fn file(&self, extent: Extent) -> ExfatFile<R> {
        ExfatFile {
            vol: ExfatVolume { src: self.src.clone(), g: self.g },
            extent,
            fat: FatCache::default(),
            checkpoints: vec![extent.first],
            cursor: (0, extent.first),
        }
    }

    pub fn open_file(&self, e: &DirEntry) -> ExfatFile<R> {
        self.file(e.extent)
    }

    pub fn read_dir(&mut self, dir: &Dir) -> Result<Vec<DirEntry>> {
        let data = self.read_dir_bytes(dir)?;
        Ok(parse_entries(&data).0)
    }

    pub fn label(&mut self) -> Result<Option<String>> {
        let root = self.root();
        let data = self.read_dir_bytes(&root)?;
        Ok(parse_entries(&data).1)
    }

    fn read_dir_bytes(&mut self, dir: &Dir) -> Result<Vec<u8>> {
        let mut ext = dir.0;
        if ext.len.is_none() {
            // Root directory: follow the chain to find its size.
            let mut cache = FatCache::default();
            let (mut n, mut c) = (1u64, ext.first);
            loop {
                let next = self.fat_entry(&mut cache, c)?;
                if next >= END_OF_CHAIN {
                    break;
                }
                if !self.g.valid_cluster(next) || n > self.g.cluster_count as u64 {
                    return Err(ExfatError::Corrupt("root directory chain"));
                }
                (n, c) = (n + 1, next);
            }
            ext.len = Some(n * self.g.cluster_bytes);
            ext.valid_len = ext.len;
        }
        let len = ext.len.unwrap();
        if len > 64 << 20 {
            return Err(ExfatError::Corrupt("directory too large"));
        }
        let mut f = self.file(ext);
        let mut buf = vec![0u8; len as usize];
        f.read_at(0, &mut buf).map_err(|_| ExfatError::Io)?;
        Ok(buf)
    }

    /// Case-insensitive (ASCII) lookup of a `/`-separated path.
    pub fn find(&mut self, path: &str) -> Result<Option<DirEntry>> {
        let mut dir = self.root();
        let mut parts = path.split('/').filter(|p| !p.is_empty()).peekable();
        while let Some(part) = parts.next() {
            let Some(e) = self.read_dir(&dir)?.into_iter().find(|e| e.name.eq_ignore_ascii_case(part)) else {
                return Ok(None);
            };
            if parts.peek().is_none() {
                return Ok(Some(e));
            }
            match self.dir_of(&e) {
                Some(d) => dir = d,
                None => return Ok(None),
            }
        }
        Ok(None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dir(Extent);

#[derive(Default)]
struct FatCache {
    sector: Option<u64>,
    buf: Vec<u8>,
}

/// Returns (entries, volume label).
fn parse_entries(d: &[u8]) -> (Vec<DirEntry>, Option<String>) {
    let (mut out, mut label) = (Vec::new(), None);
    let mut i = 0;
    while i + 32 <= d.len() {
        match d[i] {
            0x00 => break,
            0x83 => {
                let n = (d[i + 1] as usize).min(11);
                let units: Vec<u16> = (0..n).map(|k| u16le(d, i + 2 + k * 2)).collect();
                label = Some(char::decode_utf16(units).map(|c| c.unwrap_or('?')).collect());
                i += 32;
            }
            0x85 => {
                let secondary = d[i + 1] as usize;
                let end = i + 32 * (1 + secondary);
                if secondary < 2 || end > d.len() || d[i + 32] != 0xC0 {
                    i += 32;
                    continue;
                }
                let attr = u16le(d, i + 4);
                let s = i + 32;
                let (flags, name_len) = (d[s + 1], d[s + 3] as usize);
                let (valid, first, len) = (u64le(d, s + 8), u32le(d, s + 20), u64le(d, s + 24));
                let mut units = Vec::with_capacity(name_len);
                let mut k = s + 32;
                while k < end && units.len() < name_len {
                    if d[k] == 0xC1 {
                        for j in 0..15 {
                            if units.len() < name_len {
                                units.push(u16le(d, k + 2 + j * 2));
                            }
                        }
                    }
                    k += 32;
                }
                out.push(DirEntry {
                    name: char::decode_utf16(units).map(|c| c.unwrap_or('?')).collect(),
                    is_dir: attr & 0x10 != 0,
                    size: len,
                    extent: Extent { first, len: Some(len), valid_len: Some(valid.min(len)), contiguous: flags & 2 != 0 },
                });
                i = end;
            }
            _ => i += 32,
        }
    }
    (out, label)
}

/// An open file. Owns its own reader handle, so files can be used independently of the volume.
pub struct ExfatFile<R> {
    vol: ExfatVolume<R>,
    extent: Extent,
    fat: FatCache,
    /// `checkpoints[k]` = cluster at chain index `k * CHECKPOINT_EVERY`.
    checkpoints: Vec<u32>,
    /// (chain index, cluster) of the last cluster looked up.
    cursor: (u64, u32),
}

impl<R: ReadAt + Clone> ExfatFile<R> {
    pub fn size(&self) -> u64 {
        self.extent.len.unwrap_or(0)
    }

    /// The physical location of the file: (byte offset within the volume, length) runs, in file order.
    /// One run means the file is contiguous, so something without an exFAT driver (a Linux initramfs,
    /// say) can address it with a plain offset.
    pub fn extents(&mut self) -> Result<Vec<(u64, u64)>> {
        let (len, cb) = (self.size(), self.vol.g.cluster_bytes);
        let mut runs: Vec<(u64, u64)> = Vec::new();
        let mut prev: Option<u32> = None;
        for i in 0..len.div_ceil(cb) {
            let c = self.cluster_at(i)?;
            match (prev, runs.last_mut()) {
                (Some(p), Some(r)) if c == p + 1 => r.1 += cb,
                _ => runs.push((self.vol.g.cluster_pos(c), cb)),
            }
            prev = Some(c);
        }
        // The last cluster is only partly used by the file.
        let padding = len.div_ceil(cb) * cb - len;
        if let Some(last) = runs.last_mut() {
            last.1 -= padding;
        }
        Ok(runs)
    }

    /// True if the file is stored in consecutive clusters (no FAT chain needed).
    pub fn is_contiguous(&self) -> bool {
        self.extent.contiguous
    }

    fn next(&mut self, c: u32) -> Result<Option<u32>> {
        let n = self.vol.fat_entry(&mut self.fat, c)?;
        if n >= END_OF_CHAIN {
            return Ok(None);
        }
        if !self.vol.g.valid_cluster(n) {
            return Err(ExfatError::Corrupt("cluster chain"));
        }
        Ok(Some(n))
    }

    fn cluster_at(&mut self, idx: u64) -> Result<u32> {
        if self.extent.contiguous {
            return Ok(self.extent.first + idx as u32);
        }
        if idx < self.cursor.0 {
            let k = ((idx / CHECKPOINT_EVERY) as usize).min(self.checkpoints.len() - 1);
            self.cursor = (k as u64 * CHECKPOINT_EVERY, self.checkpoints[k]);
        }
        while self.cursor.0 < idx {
            let c = self.next(self.cursor.1)?.ok_or(ExfatError::Corrupt("chain shorter than file"))?;
            self.cursor = (self.cursor.0 + 1, c);
            if self.cursor.0 % CHECKPOINT_EVERY == 0 && (self.cursor.0 / CHECKPOINT_EVERY) as usize == self.checkpoints.len() {
                self.checkpoints.push(c);
            }
        }
        Ok(self.cursor.1)
    }
}

impl<R: ReadAt + Clone> ReadAt for ExfatFile<R> {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> core::result::Result<(), IoError> {
        let len = self.extent.len.ok_or(IoError)?;
        if offset.checked_add(buf.len() as u64).is_none_or(|e| e > len) {
            return Err(IoError);
        }
        let valid = self.extent.valid_len.unwrap_or(len);
        let cb = self.vol.g.cluster_bytes;
        let mut done = 0usize;
        while done < buf.len() {
            let pos = offset + done as u64;
            let want = buf.len() - done;
            if pos >= valid {
                buf[done..].fill(0); // bytes past ValidDataLength read as zero
                return Ok(());
            }
            let want = want.min((valid - pos) as usize);
            let (idx, within) = (pos / cb, pos % cb);
            let first = self.cluster_at(idx).map_err(|_| IoError)?;
            let need_clusters = (within + want as u64).div_ceil(cb);
            // Extend the run while the next cluster is physically adjacent.
            let mut run = 1u64;
            if self.extent.contiguous {
                run = need_clusters;
            } else {
                while run < need_clusters {
                    let c = self.cluster_at(idx + run).map_err(|_| IoError)?;
                    if c != first + run as u32 {
                        break;
                    }
                    run += 1;
                }
            }
            let bytes = want.min((run * cb - within) as usize);
            self.vol
                .src
                .read_at(self.vol.g.cluster_pos(first) + within, &mut buf[done..done + bytes])?;
            done += bytes;
        }
        Ok(())
    }
}
