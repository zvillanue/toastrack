//! Minimal exFAT formatter: boot regions, FAT, allocation bitmap, up-case table, root directory,
//! and a small tree of directories and files created at format time. Pure Rust, so it works on
//! every platform without `mkfs.exfat`.

use crate::{Error, Result};
use std::io::{Seek, SeekFrom, Write};

const BPS: u64 = 512;
const GIB: u64 = 1 << 30;
const ALIGN_SECTORS: u64 = 2048; // 1 MiB: FAT and cluster heap start on flash-friendly boundaries

/// Something to create at format time.
pub enum Node<'a> {
    Dir(&'a str, Vec<Node<'a>>),
    File(&'a str, &'a [u8]),
}

struct Plan {
    spc_shift: u32,
    cluster_bytes: u64,
    vol_sectors: u64,
    fat_len: u64,
    heap_off: u64,
    cluster_count: u32,
}

fn plan(vol_bytes: u64) -> Result<Plan> {
    let vol_sectors = vol_bytes / BPS;
    let cluster_bytes: u64 = if vol_bytes <= 32 * GIB { 32 * 1024 } else { 128 * 1024 };
    let spc = cluster_bytes / BPS;
    if vol_sectors < ALIGN_SECTORS * 4 {
        return Err(Error::Layout("volume too small for exFAT".into()));
    }
    let guess = (vol_sectors - ALIGN_SECTORS) / spc;
    let fat_len = ((guess + 2) * 4).div_ceil(BPS);
    let heap_off = (ALIGN_SECTORS + fat_len).div_ceil(ALIGN_SECTORS) * ALIGN_SECTORS;
    let cluster_count = (vol_sectors - heap_off) / spc;
    if cluster_count < 16 || cluster_count > 0xFFFF_FFF5 {
        return Err(Error::Layout("exFAT cluster count out of range".into()));
    }
    Ok(Plan { spc_shift: spc.trailing_zeros(), cluster_bytes, vol_sectors, fat_len, heap_off, cluster_count: cluster_count as u32 })
}

/// Case-folding table: ASCII only, in the compressed form the specification allows
/// (0xFFFF, n = "next n characters map to themselves").
fn upcase_table() -> Vec<u8> {
    let mut t: Vec<u16> = Vec::new();
    t.extend([0xFFFF, 0x0061]);
    t.extend((b'A'..=b'Z').map(u16::from));
    t.extend([0xFFFF, (0x10000u32 - 0x7B) as u16]);
    t.iter().flat_map(|u| u.to_le_bytes()).collect()
}

fn upcase_char(u: u16) -> u16 {
    if (b'a' as u16..=b'z' as u16).contains(&u) { u - 32 } else { u }
}

fn table_checksum(d: &[u8]) -> u32 {
    d.iter().fold(0u32, |s, &b| (s << 31 | s >> 1).wrapping_add(b as u32))
}

fn set_checksum(set: &[u8]) -> u16 {
    let mut s = 0u16;
    for (i, &b) in set.iter().enumerate() {
        if i == 2 || i == 3 {
            continue;
        }
        s = (s << 15 | s >> 1).wrapping_add(b as u16);
    }
    s
}

fn name_hash(units: &[u16]) -> u16 {
    let mut h = 0u16;
    for &u in units {
        for b in upcase_char(u).to_le_bytes() {
            h = (h << 15 | h >> 1).wrapping_add(b as u16);
        }
    }
    h
}

/// Current time in exFAT's packed DOS format (UTC).
fn dos_now() -> u32 {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // civil-from-days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = (yoe + era * 400 + i64::from(m <= 2)) as u32;
    let y = y.clamp(1980, 2107) - 1980;
    (y << 25) | (m << 21) | (d << 16) | ((rem / 3600) as u32) << 11 | ((rem % 3600 / 60) as u32) << 5 | (rem % 60 / 2) as u32
}

fn entry_set(name: &str, is_dir: bool, first_cluster: u32, len: u64, now: u32) -> Vec<u8> {
    let units: Vec<u16> = name.encode_utf16().collect();
    assert!(units.len() <= 255 && !units.is_empty());
    let names = units.len().div_ceil(15);
    let mut set = vec![0u8; 32 * (2 + names)];
    set[0] = 0x85;
    set[1] = (1 + names) as u8;
    set[4..6].copy_from_slice(&(if is_dir { 0x10u16 } else { 0x20 }).to_le_bytes());
    for o in [8, 12, 16] {
        set[o..o + 4].copy_from_slice(&now.to_le_bytes());
    }
    let s = &mut set[32..64];
    s[0] = 0xC0;
    s[1] = if first_cluster == 0 { 0x01 } else { 0x03 }; // allocation possible (+ NoFatChain)
    s[3] = units.len() as u8;
    s[4..6].copy_from_slice(&name_hash(&units).to_le_bytes());
    s[8..16].copy_from_slice(&len.to_le_bytes());
    s[20..24].copy_from_slice(&first_cluster.to_le_bytes());
    s[24..32].copy_from_slice(&len.to_le_bytes());
    for (i, chunk) in units.chunks(15).enumerate() {
        let e = &mut set[64 + 32 * i..96 + 32 * i];
        e[0] = 0xC1;
        for (j, u) in chunk.iter().enumerate() {
            e[2 + 2 * j..4 + 2 * j].copy_from_slice(&u.to_le_bytes());
        }
    }
    let cs = set_checksum(&set);
    set[2..4].copy_from_slice(&cs.to_le_bytes());
    set
}

struct Heap {
    cluster_bytes: u64,
    next: u32,
    /// (first cluster, data) for every object placed in the heap beyond the system files.
    blobs: Vec<(u32, Vec<u8>)>,
}

impl Heap {
    fn alloc(&mut self, bytes: u64) -> u32 {
        let c = self.next;
        self.next += bytes.div_ceil(self.cluster_bytes).max(1) as u32;
        c
    }

    /// Place `nodes` as the contents of a directory; returns the directory's entry sets.
    fn place(&mut self, nodes: &[Node], now: u32) -> Result<Vec<u8>> {
        let mut entries = Vec::new();
        for n in nodes {
            match n {
                Node::File(name, data) => {
                    let first = if data.is_empty() { 0 } else { self.alloc(data.len() as u64) };
                    if first != 0 {
                        self.blobs.push((first, data.to_vec()));
                    }
                    entries.extend(entry_set(name, false, first, data.len() as u64, now));
                }
                Node::Dir(name, children) => {
                    let first = self.alloc(self.cluster_bytes); // reserve before children so the numbering is stable
                    let content = self.place(children, now)?;
                    if content.len() as u64 > self.cluster_bytes {
                        return Err(Error::Layout("too many entries for a format-time directory".into()));
                    }
                    self.blobs.push((first, content));
                    entries.extend(entry_set(name, true, first, self.cluster_bytes, now));
                }
            }
        }
        Ok(entries)
    }
}

/// Format `dev` (positioned at the start of the volume) as exFAT.
/// `partition_start` is only recorded in the boot sector (the volume's offset on its disk).
pub fn format<D: Write + Seek>(dev: &mut D, vol_bytes: u64, partition_start: u64, label: &str, serial: u32, tree: &[Node]) -> Result<()> {
    let p = plan(vol_bytes)?;
    let now = dos_now();
    let cb = p.cluster_bytes;

    // Cluster heap layout: bitmap, up-case table, root directory, then the format-time tree.
    let bitmap_bytes = (p.cluster_count as u64).div_ceil(8);
    let up = upcase_table();
    let mut heap = Heap { cluster_bytes: cb, next: 2, blobs: Vec::new() };
    let bitmap_first = heap.alloc(bitmap_bytes);
    let up_first = heap.alloc(up.len() as u64);
    let root_first = heap.alloc(cb);
    let mut root = Vec::new();
    let label_units: Vec<u16> = label.encode_utf16().collect();
    if label_units.len() > 11 {
        return Err(Error::Layout("exFAT label is at most 11 characters".into()));
    }
    let mut e = [0u8; 32];
    e[0] = 0x83;
    e[1] = label_units.len() as u8;
    for (i, u) in label_units.iter().enumerate() {
        e[2 + 2 * i..4 + 2 * i].copy_from_slice(&u.to_le_bytes());
    }
    root.extend(e);
    let mut e = [0u8; 32];
    e[0] = 0x81;
    e[20..24].copy_from_slice(&bitmap_first.to_le_bytes());
    e[24..32].copy_from_slice(&bitmap_bytes.to_le_bytes());
    root.extend(e);
    let mut e = [0u8; 32];
    e[0] = 0x82;
    e[4..8].copy_from_slice(&table_checksum(&up).to_le_bytes());
    e[20..24].copy_from_slice(&up_first.to_le_bytes());
    e[24..32].copy_from_slice(&(up.len() as u64).to_le_bytes());
    root.extend(e);
    root.extend(heap.place(tree, now)?);
    if root.len() as u64 > cb {
        return Err(Error::Layout("too many entries for the root directory".into()));
    }
    let used = heap.next - 2; // clusters in use
    if used > p.cluster_count {
        return Err(Error::Layout("volume too small for the initial files".into()));
    }

    // FAT: media descriptor, then end-of-chain marks; multi-cluster objects (the bitmap) get real chains.
    let mut fat = vec![0u8; (p.fat_len * BPS) as usize];
    let mut set = |i: u32, v: u32| fat[i as usize * 4..i as usize * 4 + 4].copy_from_slice(&v.to_le_bytes());
    set(0, 0xFFFF_FFF8);
    set(1, 0xFFFF_FFFF);
    for c in 2..heap.next {
        set(c, if c + 1 < heap.next && (bitmap_first..bitmap_first + bitmap_bytes.div_ceil(cb) as u32 - 1).contains(&c) { c + 1 } else { 0xFFFF_FFFF });
    }

    // Boot sector and the rest of the boot region.
    let mut region = vec![0u8; 12 * BPS as usize];
    {
        let b = &mut region[..512];
        b[..3].copy_from_slice(&[0xEB, 0x76, 0x90]);
        b[3..11].copy_from_slice(b"EXFAT   ");
        b[0x40..0x48].copy_from_slice(&(partition_start / BPS).to_le_bytes());
        b[0x48..0x50].copy_from_slice(&p.vol_sectors.to_le_bytes());
        b[0x50..0x54].copy_from_slice(&(ALIGN_SECTORS as u32).to_le_bytes());
        b[0x54..0x58].copy_from_slice(&(p.fat_len as u32).to_le_bytes());
        b[0x58..0x5C].copy_from_slice(&(p.heap_off as u32).to_le_bytes());
        b[0x5C..0x60].copy_from_slice(&p.cluster_count.to_le_bytes());
        b[0x60..0x64].copy_from_slice(&root_first.to_le_bytes());
        b[0x64..0x68].copy_from_slice(&serial.to_le_bytes());
        b[0x68..0x6A].copy_from_slice(&0x0100u16.to_le_bytes());
        b[0x6C] = 9;
        b[0x6D] = p.spc_shift as u8;
        b[0x6E] = 1;
        b[0x6F] = 0x80;
        b[0x70] = 0xFF; // percent in use: unknown
        b[0x78..0x1FE].fill(0xF4);
        b[510] = 0x55;
        b[511] = 0xAA;
    }
    for s in 1..9 {
        region[s * 512 + 508..s * 512 + 512].copy_from_slice(&[0, 0, 0x55, 0xAA]);
    }
    let mut cs = 0u32;
    for (i, &b) in region[..11 * 512].iter().enumerate() {
        if i == 106 || i == 107 || i == 112 {
            continue;
        }
        cs = (cs << 31 | cs >> 1).wrapping_add(b as u32);
    }
    for i in 0..128 {
        region[11 * 512 + i * 4..11 * 512 + i * 4 + 4].copy_from_slice(&cs.to_le_bytes());
    }

    let mut put = |off: u64, data: &[u8]| -> Result<()> {
        dev.seek(SeekFrom::Start(off))?;
        dev.write_all(data)?;
        Ok(())
    };
    put(0, &region)?;
    put(12 * BPS, &region)?; // backup boot region
    put(ALIGN_SECTORS * BPS, &fat)?;

    let cluster_pos = |c: u32| p.heap_off * BPS + (c as u64 - 2) * cb;
    let mut bitmap = vec![0u8; bitmap_bytes as usize];
    for c in 0..used as usize {
        bitmap[c / 8] |= 1 << (c % 8);
    }
    put(cluster_pos(bitmap_first), &bitmap)?;
    put(cluster_pos(up_first), &up)?;
    root.resize(cb as usize, 0);
    put(cluster_pos(root_first), &root)?;
    for (first, mut data) in heap.blobs {
        data.resize(data.len().div_ceil(cb as usize) * cb as usize, 0);
        put(cluster_pos(first), &data)?;
    }
    Ok(())
}
