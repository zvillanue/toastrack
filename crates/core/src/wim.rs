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
