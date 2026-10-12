//! Windows Imaging (WIM) header and XML metadata - what is needed to describe a boot.wim / install.wim.

use crate::io::ReadAt;
use alloc::{string::String, vec, vec::Vec};

pub const HEADER_LEN: usize = 208;

pub const FLAG_COMPRESSED: u32 = 0x2;
pub const FLAG_XPRESS: u32 = 0x20000;
pub const FLAG_LZX: u32 = 0x40000;
pub const FLAG_LZMS: u32 = 0x80000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WimInfo {
    pub version: u32,
    pub flags: u32,
    pub chunk_size: u32,
    pub part_number: u16,
    pub total_parts: u16,
    pub image_count: u32,
    pub boot_index: u32,
    /// (stored size, original size, offset) of the XML data resource.
    pub xml: (u64, u64, u64),
    pub images: Vec<WimImage>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WimImage {
    pub index: u32,
    pub name: String,
    pub description: String,
    pub build: Option<u32>,
    pub arch: Option<u32>,
}

impl WimInfo {
    pub fn compression(&self) -> &'static str {
        if self.flags & FLAG_COMPRESSED == 0 {
            "none"
        } else if self.flags & FLAG_LZX != 0 {
            "LZX"
        } else if self.flags & FLAG_XPRESS != 0 {
            "XPRESS"
        } else if self.flags & FLAG_LZMS != 0 {
            "LZMS (solid ESD)"
        } else {
            "unknown"
        }
    }
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// A resource header: 7-byte stored size + 1 flag byte, 8-byte offset, 8-byte original size.
fn reshdr(b: &[u8], o: usize) -> (u64, u64, u64) {
    let size = u64::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3], b[o + 4], b[o + 5], b[o + 6], 0]);
    let off = u64::from_le_bytes(b[o + 8..o + 16].try_into().unwrap());
    let orig = u64::from_le_bytes(b[o + 16..o + 24].try_into().unwrap());
    (size, orig, off)
}

fn tag<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let open = alloc::format!("<{name}>");
    let close = alloc::format!("</{name}>");
    let a = s.find(&open)? + open.len();
    let b = s[a..].find(&close)? + a;
    Some(s[a..b].trim())
}

fn parse_xml(xml: &str) -> Vec<WimImage> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<IMAGE INDEX=\"") {
        let after = &rest[i + 14..];
        let end = after.find("</IMAGE>").unwrap_or(after.len());
        let body = &after[..end];
        let index = body.split('"').next().and_then(|n| n.parse().ok()).unwrap_or(0);
        out.push(WimImage {
            index,
            name: tag(body, "NAME").unwrap_or("").into(),
            description: tag(body, "DESCRIPTION").unwrap_or("").into(),
            build: tag(body, "BUILD").and_then(|b| b.parse().ok()),
            arch: tag(body, "ARCH").and_then(|b| b.parse().ok()),
        });
        rest = &after[end..];
    }
    out
}

/// Parse the header (and the uncompressed XML metadata, if present) from the start of a WIM.
pub fn read_info<R: ReadAt>(r: &mut R, file_len: u64) -> Option<WimInfo> {
    let mut h = vec![0u8; HEADER_LEN];
    r.read_at(0, &mut h).ok()?;
    if &h[..8] != b"MSWIM\0\0\0" || u32le(&h, 8) as usize != HEADER_LEN {
        return None;
    }
    let xml = reshdr(&h, 72);
    let mut info = WimInfo {
        version: u32le(&h, 12),
        flags: u32le(&h, 16),
        chunk_size: u32le(&h, 20),
        part_number: u16::from_le_bytes([h[40], h[41]]),
        total_parts: u16::from_le_bytes([h[42], h[43]]),
        image_count: u32le(&h, 44),
        boot_index: u32le(&h, 120),
        xml,
        images: Vec::new(),
    };
    let (stored, orig, off) = xml;
    // The XML resource is stored uncompressed in practice (stored == original size).
    if stored == orig && stored > 2 && stored <= 4 << 20 && off.checked_add(stored).is_some_and(|e| e <= file_len) {
        let mut raw = vec![0u8; stored as usize];
        if r.read_at(off, &mut raw).is_ok() {
            let units: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let text: String = char::decode_utf16(units.into_iter().skip_while(|&u| u == 0xFEFF)).map(|c| c.unwrap_or('?')).collect();
            info.images = parse_xml(&text);
        }
    }
    Some(info)
}

// ------------------------------------------------------------------ resources, lookup table, directory tree

use crate::{io::IoError, lzx};
use sha1::{Digest, Sha1};

const RES_METADATA: u8 = 0x02;
const RES_COMPRESSED: u8 = 0x04;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resource {
    /// Bytes stored in the file (for a compressed resource this includes its chunk table).
    pub stored: u64,
    pub flags: u8,
    pub offset: u64,
    /// Size after decompression.
    pub orig: u64,
}

impl Resource {
    fn parse(b: &[u8], o: usize) -> Self {
        let (stored, orig, offset) = reshdr(b, o);
        Resource { stored, flags: b[o + 7], offset, orig }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupEntry {
    pub res: Resource,
    pub part: u16,
    pub refcount: u32,
    pub sha1: [u8; 20],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WimError {
    Io,
    NotWim,
    Unsupported(&'static str),
    Corrupt(&'static str),
    HashMismatch,
    Lzx(lzx::LzxError),
}

impl From<IoError> for WimError {
    fn from(_: IoError) -> Self {
        WimError::Io
    }
}

impl core::fmt::Display for WimError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            WimError::Io => f.write_str("read error"),
            WimError::NotWim => f.write_str("not a WIM file"),
            WimError::Unsupported(w) => write!(f, "unsupported WIM feature: {w}"),
            WimError::Corrupt(w) => write!(f, "corrupt WIM: {w}"),
            WimError::HashMismatch => f.write_str("resource does not match its recorded SHA-1"),
            WimError::Lzx(e) => write!(f, "LZX decompression failed: {e:?}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dentry {
    pub name: String,
    pub is_dir: bool,
    pub attributes: u32,
    /// SHA-1 of the file's unnamed data stream (all zeros = empty).
    pub hash: [u8; 20],
    subdir: u64,
}

pub struct Wim<R> {
    r: R,
    pub info: WimInfo,
    lookup: Vec<LookupEntry>,
}

impl<R: ReadAt> Wim<R> {
    pub fn open(mut r: R, file_len: u64) -> core::result::Result<Self, WimError> {
        let info = read_info(&mut r, file_len).ok_or(WimError::NotWim)?;
        if info.total_parts != 1 {
            return Err(WimError::Unsupported("split WIM (.swm)"));
        }
        let mut h = vec![0u8; HEADER_LEN];
        r.read_at(0, &mut h)?;
        let table = Resource::parse(&h, 48);
        let mut w = Self { r, info, lookup: Vec::new() };
        let raw = w.read_resource(&table, false)?;
        w.lookup = raw
            .chunks_exact(50)
            .map(|e| LookupEntry {
                res: Resource::parse(e, 0),
                part: u16::from_le_bytes([e[24], e[25]]),
                refcount: u32::from_le_bytes([e[26], e[27], e[28], e[29]]),
                sha1: e[30..50].try_into().unwrap(),
            })
            .collect();
        Ok(w)
    }

    pub fn lookup(&self) -> &[LookupEntry] {
        &self.lookup
    }

    /// Read a whole resource (decompressing it if needed).
    pub fn read_resource(&mut self, res: &Resource, verify: bool) -> core::result::Result<Vec<u8>, WimError> {
        let _ = verify;
        if res.orig > 1 << 32 {
            return Err(WimError::Unsupported("resource over 4 GiB"));
        }
        let mut out = vec![0u8; res.orig as usize];
        if res.flags & RES_COMPRESSED == 0 {
            self.r.read_at(res.offset, &mut out)?;
            return Ok(out);
        }
        let chunk = self.info.chunk_size as u64;
        let n = res.orig.div_ceil(chunk).max(1);
        let entry = if res.orig > 0xFFFF_FFFF { 8 } else { 4 };
        let table_len = (n - 1) * entry;
        let mut tab = vec![0u8; table_len as usize];
        self.r.read_at(res.offset, &mut tab)?;
        let start = |i: u64| -> u64 {
            if i == 0 {
                0
            } else if entry == 4 {
                let o = ((i - 1) * 4) as usize;
                u32::from_le_bytes([tab[o], tab[o + 1], tab[o + 2], tab[o + 3]]) as u64
            } else {
                let o = ((i - 1) * 8) as usize;
                u64::from_le_bytes(tab[o..o + 8].try_into().unwrap())
            }
        };
        let data_len = res.stored.checked_sub(table_len).ok_or(WimError::Corrupt("chunk table"))?;
        for i in 0..n {
            let (a, z) = (start(i), if i + 1 == n { data_len } else { start(i + 1) });
            let raw_len = if i + 1 == n { res.orig - i * chunk } else { chunk };
            let clen = z.checked_sub(a).ok_or(WimError::Corrupt("chunk offsets"))?;
            let mut c = vec![0u8; clen as usize];
            self.r.read_at(res.offset + table_len + a, &mut c)?;
            let dst = &mut out[(i * chunk) as usize..(i * chunk + raw_len) as usize];
            if clen == raw_len {
                dst.copy_from_slice(&c); // stored uncompressed
            } else if self.info.flags & FLAG_LZX != 0 {
                let d = lzx::decompress(&c, raw_len as usize, self.info.chunk_size).map_err(WimError::Lzx)?;
                dst.copy_from_slice(&d);
            } else {
                return Err(WimError::Unsupported("compression other than LZX"));
            }
        }
        Ok(out)
    }

    fn entry_for_hash(&self, h: &[u8; 20]) -> Option<LookupEntry> {
        self.lookup.iter().find(|e| &e.sha1 == h && e.res.flags & RES_METADATA == 0).cloned()
    }

    /// Size of the stream with this hash.
    pub fn stream_size(&self, h: &[u8; 20]) -> Option<u64> {
        self.entry_for_hash(h).map(|e| e.res.orig)
    }

    /// Read a stream by its SHA-1, checking the hash of what was read.
    pub fn read_stream(&mut self, h: &[u8; 20]) -> core::result::Result<Vec<u8>, WimError> {
        let e = self.entry_for_hash(h).ok_or(WimError::Corrupt("stream not in lookup table"))?;
        let d = self.read_resource(&e.res, true)?;
        if Sha1::digest(&d).as_slice() != h {
            return Err(WimError::HashMismatch);
        }
        Ok(d)
    }

    /// The (decompressed, hash-verified) metadata resource of image `index` (1-based).
    pub fn metadata(&mut self, index: u32) -> core::result::Result<Vec<u8>, WimError> {
        let e = self
            .lookup
            .iter()
            .filter(|e| e.res.flags & RES_METADATA != 0)
            .nth(index.checked_sub(1).ok_or(WimError::Corrupt("image index 0"))? as usize)
            .cloned()
            .ok_or(WimError::Corrupt("no such image"))?;
        let d = self.read_resource(&e.res, true)?;
        if Sha1::digest(&d).as_slice() != e.sha1 {
            return Err(WimError::HashMismatch);
        }
        Ok(d)
    }
}

fn u64at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Parse the directory entries starting at `off` of a metadata resource (until the zero-length terminator).
fn parse_dentries(m: &[u8], mut off: usize) -> core::result::Result<Vec<Dentry>, WimError> {
    let mut out = Vec::new();
    loop {
        if off + 8 > m.len() {
            return Err(WimError::Corrupt("directory runs past the metadata"));
        }
        let len = u64at(m, off) as usize;
        if len == 0 {
            return Ok(out);
        }
        if len < 102 || off + len > m.len() {
            return Err(WimError::Corrupt("directory entry length"));
        }
        let e = &m[off..off + len];
        // Fixed part is 102 bytes: ... num_alternate_streams @96, short_name_nbytes @98, file_name_nbytes @100.
        let name_bytes = u16::from_le_bytes([e[100], e[101]]) as usize;
        let name_raw = e.get(102..102 + name_bytes).ok_or(WimError::Corrupt("file name"))?;
        let units: Vec<u16> = name_raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let attributes = u32::from_le_bytes([e[8], e[9], e[10], e[11]]);
        out.push(Dentry {
            name: char::decode_utf16(units).map(|c| c.unwrap_or('?')).collect(),
            is_dir: attributes & 0x10 != 0,
            attributes,
            hash: e[64..84].try_into().unwrap(),
            subdir: u64at(e, 16),
        });
        off += len.div_ceil(8) * 8;
    }
}

/// A parsed image directory tree (from the metadata resource).
pub struct ImageTree {
    meta: Vec<u8>,
    root: Dentry,
}

impl ImageTree {
    pub fn new(meta: Vec<u8>) -> core::result::Result<Self, WimError> {
        if meta.len() < 8 {
            return Err(WimError::Corrupt("metadata too short"));
        }
        let sec = u32::from_le_bytes([meta[0], meta[1], meta[2], meta[3]]) as usize;
        let root_off = sec.max(8).div_ceil(8) * 8;
        let root = parse_dentries(&meta, root_off)?.into_iter().next().ok_or(WimError::Corrupt("no root directory"))?;
        Ok(Self { meta, root })
    }

    pub fn children(&self, dir: &Dentry) -> core::result::Result<Vec<Dentry>, WimError> {
        if !dir.is_dir || dir.subdir == 0 {
            return Ok(Vec::new());
        }
        parse_dentries(&self.meta, dir.subdir as usize)
    }

    pub fn root(&self) -> &Dentry {
        &self.root
    }

    /// Case-insensitive lookup of a `/`-separated path ("" = root).
    pub fn find(&self, path: &str) -> core::result::Result<Option<Dentry>, WimError> {
        let mut cur = self.root.clone();
        for part in path.split(['/', '\\']).filter(|p| !p.is_empty()) {
            match self.children(&cur)?.into_iter().find(|e| e.name.eq_ignore_ascii_case(part)) {
                Some(e) => cur = e,
                None => return Ok(None),
            }
        }
        Ok(Some(cur))
    }
}
