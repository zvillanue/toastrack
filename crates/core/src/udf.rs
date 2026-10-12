//! Read-only UDF (ECMA-167 / OSTA UDF 1.02-2.01), enough to list and read the files of Windows install media.
//!
//! Windows ISOs are UDF; the ISO 9660 side is only a stub. Supported: a single physical partition,
//! File Entries and Extended File Entries with short, long or embedded allocation descriptors. Not supported:
//! virtual/sparable/metadata partition maps (UDF 2.50 metadata, VAT) - reported as an error, never guessed.

use crate::io::{IoError, ReadAt};
use alloc::{string::String, vec, vec::Vec};

const SECTOR: u64 = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UdfError {
    Io,
    NotUdf,
    Unsupported(&'static str),
    Corrupt(&'static str),
}

impl From<IoError> for UdfError {
    fn from(_: IoError) -> Self {
        UdfError::Io
    }
}

impl core::fmt::Display for UdfError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            UdfError::Io => f.write_str("read error"),
            UdfError::NotUdf => f.write_str("not a UDF volume"),
            UdfError::Unsupported(w) => write!(f, "unsupported UDF feature: {w}"),
            UdfError::Corrupt(w) => write!(f, "corrupt UDF: {w}"),
        }
    }
}

type Result<T> = core::result::Result<T, UdfError>;

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// One run of file data: byte offset on the device (None = a hole that reads as zeros) and length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Run {
    pos: Option<u64>,
    len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Data {
    Embedded(Vec<u8>),
    Runs(Vec<Run>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdfEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Logical block (partition-relative) of this entry's File Entry.
    icb: u32,
}

pub struct UdfVolume<R> {
    src: R,
    block: u64,
    part_start: u64, // byte offset of the partition on the device
    root_icb: u32,
}

impl<R: ReadAt> UdfVolume<R> {
    pub fn open(mut src: R) -> Result<Self> {
        // Anchor Volume Descriptor Pointer at sector 256.
        let mut s = vec![0u8; SECTOR as usize];
        src.read_at(256 * SECTOR, &mut s).map_err(|_| UdfError::NotUdf)?;
        if u16le(&s, 0) != 2 {
            return Err(UdfError::NotUdf);
        }
        let (vds_len, vds_loc) = (u32le(&s, 16) as u64, u32le(&s, 20) as u64);

        let (mut part_no, mut part_start, mut block, mut fsd) = (None, 0u64, SECTOR, None);
        let mut maps: Vec<(u8, u16)> = Vec::new();
        for i in 0..(vds_len / SECTOR).min(64) {
            src.read_at((vds_loc + i) * SECTOR, &mut s)?;
            match u16le(&s, 0) {
                5 => {
                    part_no = Some(u16le(&s, 22));
                    part_start = u32le(&s, 188) as u64;
                }
                6 => {
                    block = u32le(&s, 212) as u64;
                    // File Set Descriptor: long_ad at 248 (length, lbn, partition ref).
                    fsd = Some((u32le(&s, 252), u16le(&s, 256)));
                    let n = u32le(&s, 268) as usize;
                    let mut o = 440;
                    for _ in 0..n.min(8) {
                        let (t, l) = (s[o], s[o + 1] as usize);
                        maps.push((t, if t == 1 { u16le(&s, o + 4) } else { 0 }));
                        o += l.max(2);
                    }
                }
                8 => break,
                _ => {}
            }
        }
        if block != SECTOR {
            return Err(UdfError::Unsupported("logical block size other than 2048"));
        }
        if maps.iter().any(|(t, _)| *t != 1) {
            return Err(UdfError::Unsupported("virtual, sparable or metadata partition map (UDF 2.50+)"));
        }
        let (Some(_), Some((fsd_lbn, _))) = (part_no, fsd) else { return Err(UdfError::Corrupt("missing partition or logical volume descriptor")) };

        let mut v = Self { src, block, part_start: part_start * SECTOR, root_icb: 0 };
        // File Set Descriptor -> root directory ICB.
        let mut f = vec![0u8; SECTOR as usize];
        v.src.read_at(v.part_start + fsd_lbn as u64 * v.block, &mut f)?;
        if u16le(&f, 0) != 256 {
            return Err(UdfError::Corrupt("file set descriptor"));
        }
        v.root_icb = u32le(&f, 404);
        Ok(v)
    }

    fn read_icb(&mut self, lbn: u32) -> Result<(bool, u64, Data)> {
        let mut b = vec![0u8; self.block as usize];
        self.src.read_at(self.part_start + lbn as u64 * self.block, &mut b)?;
        let tag = u16le(&b, 0);
        let (ea_off, ad_len_off, ad_base) = match tag {
            261 => (168, 172, 176),
            266 => (208, 212, 216),
            _ => return Err(UdfError::Corrupt("expected a file entry")),
        };
        let is_dir = b[27] == 4;
        let info_len = u64le(&b, 56);
        let (ea, adl) = (u32le(&b, ea_off) as usize, u32le(&b, ad_len_off) as usize);
        let ads = b.get(ad_base + ea..ad_base + ea + adl).ok_or(UdfError::Corrupt("allocation descriptors"))?.to_vec();
        let kind = u16le(&b, 34) & 7;
        let data = match kind {
            3 => Data::Embedded(ads[..(info_len as usize).min(ads.len())].to_vec()),
            0 | 1 => {
                let stride = if kind == 0 { 8 } else { 16 };
                let mut runs = Vec::new();
                for ad in ads.chunks_exact(stride) {
                    let (raw, lbn) = (u32le(ad, 0), u32le(ad, 4));
                    let (typ, len) = (raw >> 30, (raw & 0x3FFF_FFFF) as u64);
                    match typ {
                        0 => runs.push(Run { pos: Some(self.part_start + lbn as u64 * self.block), len }),
                        1 | 2 => runs.push(Run { pos: None, len }), // allocated-not-recorded / unallocated: zeros
                        _ => return Err(UdfError::Unsupported("continuation extents (very fragmented file)")),
                    }
                }
                Data::Runs(runs)
            }
            _ => return Err(UdfError::Unsupported("extended allocation descriptors")),
        };
        Ok((is_dir, info_len, data))
    }

    fn read_data(&mut self, data: &Data, size: u64, offset: u64, buf: &mut [u8]) -> Result<()> {
        if offset.checked_add(buf.len() as u64).is_none_or(|e| e > size) {
            return Err(UdfError::Io);
        }
        match data {
            Data::Embedded(d) => buf.copy_from_slice(&d[offset as usize..offset as usize + buf.len()]),
            Data::Runs(runs) => {
                let (mut at, mut done) = (0u64, 0usize);
                for r in runs {
                    if done == buf.len() {
                        break;
                    }
                    let want = offset + done as u64;
                    if want >= at + r.len {
                        at += r.len;
                        continue;
                    }
                    let within = want - at;
                    let n = ((r.len - within) as usize).min(buf.len() - done);
                    match r.pos {
                        Some(p) => self.src.read_at(p + within, &mut buf[done..done + n])?,
                        None => buf[done..done + n].fill(0),
                    }
                    done += n;
                    at += r.len;
                }
                if done != buf.len() {
                    return Err(UdfError::Corrupt("file shorter than its recorded length"));
                }
            }
        }
        Ok(())
    }

    pub fn root(&self) -> UdfEntry {
        UdfEntry { name: String::new(), is_dir: true, size: 0, icb: self.root_icb }
    }

    pub fn read_dir(&mut self, dir: &UdfEntry) -> Result<Vec<UdfEntry>> {
        let (is_dir, size, data) = self.read_icb(dir.icb)?;
        if !is_dir || size > 16 << 20 {
            return Err(UdfError::Corrupt("directory"));
        }
        let mut d = vec![0u8; size as usize];
        self.read_data(&data, size, 0, &mut d)?;
        let mut out = Vec::new();
        let mut i = 0usize;
        while i + 38 <= d.len() {
            if u16le(&d, i) != 257 {
                break;
            }
            let (chars, l_fi, l_iu) = (d[i + 18], d[i + 19] as usize, u16le(&d, i + 36) as usize);
            let icb = u32le(&d, i + 24);
            let name_at = i + 38 + l_iu;
            let raw = d.get(name_at..name_at + l_fi).ok_or(UdfError::Corrupt("file identifier"))?;
            i += (38 + l_iu + l_fi).div_ceil(4) * 4;
            if chars & 0x08 != 0 || chars & 0x04 != 0 || raw.is_empty() {
                continue; // parent link or deleted
            }
            let name: String = match raw[0] {
                8 => raw[1..].iter().map(|&b| b as char).collect(),
                16 => char::decode_utf16(raw[1..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]]))).map(|c| c.unwrap_or('?')).collect(),
                _ => continue,
            };
            let (is_dir, size, _) = self.read_icb(icb)?;
            out.push(UdfEntry { name, is_dir, size, icb });
        }
        Ok(out)
    }

    /// Case-insensitive lookup of a `/`-separated path.
    pub fn find(&mut self, path: &str) -> Result<Option<UdfEntry>> {
        let mut cur = self.root();
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

    /// Read `buf.len()` bytes of a file starting at `offset`.
    pub fn read_file_at(&mut self, e: &UdfEntry, offset: u64, buf: &mut [u8]) -> Result<()> {
        let (_, size, data) = self.read_icb(e.icb)?;
        self.read_data(&data, size, offset, buf)
    }

    pub fn read_file(&mut self, e: &UdfEntry, limit: u64) -> Result<Vec<u8>> {
        let n = e.size.min(limit);
        let mut v = vec![0u8; n as usize];
        self.read_file_at(e, 0, &mut v)?;
        Ok(v)
    }
}

/// A file inside a UDF volume as a [`ReadAt`] source (e.g. to feed the WIM parser).
pub struct UdfFileReader<'a, R> {
    pub vol: &'a mut UdfVolume<R>,
    pub entry: UdfEntry,
}

impl<R: ReadAt> ReadAt for UdfFileReader<'_, R> {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> core::result::Result<(), IoError> {
        self.vol.read_file_at(&self.entry, offset, buf).map_err(|_| IoError)
    }
}
