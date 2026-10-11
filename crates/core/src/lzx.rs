//! LZX decompression as used for WIM resources (each chunk of the resource is an independent LZX stream).
//!
//! Follows the LZX format (window 2^15..2^21, 16-bit little-endian bit reader, verbatim / aligned / uncompressed
//! blocks, three repeat offsets, E8 call translation with the WIM file size of 12,000,000).

use alloc::{vec, vec::Vec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LzxError {
    BadWindow,
    BadBlock,
    BadHuffman,
    Truncated,
    BadMatch,
}

type Result<T> = core::result::Result<T, LzxError>;

const MIN_MATCH: usize = 2;
const NUM_CHARS: usize = 256;
const NUM_LEN_SYMS: usize = 249;
const NUM_ALIGNED: usize = 8;
const PRE_SYMS: usize = 20;
const MAX_BITS: usize = 16;
const WIM_E8_FILE_SIZE: i64 = 12_000_000;

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    buf: u64,
    n: u32,
    consumed: u64, // total bits taken, to find 16-bit alignment
}

impl<'a> Bits<'a> {
    fn new(d: &'a [u8]) -> Self {
        Self { d, pos: 0, buf: 0, n: 0, consumed: 0 }
    }
    fn fill(&mut self, need: u32) {
        while self.n < need {
            let w = match self.d.get(self.pos..self.pos + 2) {
                Some(b) => u16::from_le_bytes([b[0], b[1]]),
                None => 0, // reading past the end yields zeros; callers check `overrun`
            };
            self.pos += 2;
            self.buf = (self.buf << 16) | w as u64;
            self.n += 16;
        }
    }
    fn peek(&mut self, k: u32) -> u32 {
        if k == 0 {
            return 0;
        }
        self.fill(k);
        ((self.buf >> (self.n - k)) & ((1u64 << k) - 1)) as u32
    }
    fn take(&mut self, k: u32) -> u32 {
        let v = self.peek(k);
        self.n -= k;
        self.consumed += k as u64;
        v
    }
    fn overrun(&self) -> bool {
        self.pos > self.d.len() + 2 && (self.pos - self.d.len()) * 8 > self.n as usize + 32
    }
}

/// Canonical Huffman decoder (codes assigned by length, then symbol value).
struct Huff {
    count: [u16; MAX_BITS + 1],
    symbols: Vec<u16>,
}

impl Huff {
    fn new(lens: &[u8]) -> Result<Self> {
        let mut count = [0u16; MAX_BITS + 1];
        for &l in lens {
            count[l as usize] += 1;
        }
        count[0] = 0;
        // Reject over-subscribed sets; incomplete sets are allowed (unused codes are simply never seen).
        let mut left: i32 = 1;
        for l in 1..=MAX_BITS {
            left = (left << 1) - count[l] as i32;
            if left < 0 {
                return Err(LzxError::BadHuffman);
            }
        }
        let mut offs = [0u16; MAX_BITS + 2];
        for l in 1..=MAX_BITS {
            offs[l + 1] = offs[l] + count[l];
        }
        let mut symbols = vec![0u16; lens.len()];
        for (s, &l) in lens.iter().enumerate() {
            if l != 0 {
                symbols[offs[l as usize] as usize] = s as u16;
                offs[l as usize] += 1;
            }
        }
        Ok(Self { count, symbols })
    }

    fn decode(&self, b: &mut Bits) -> Result<usize> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for l in 1..=MAX_BITS {
            code |= b.take(1) as i32;
            let c = self.count[l] as i32;
            if code - c < first {
                return Ok(self.symbols[(index + (code - first)) as usize] as usize);
            }
            index += c;
            first += c;
            first <<= 1;
            code <<= 1;
        }
        Err(LzxError::BadHuffman)
    }
}

fn position_slots(window_order: u32) -> Result<usize> {
    Ok(match window_order {
        15 => 30,
        16 => 32,
        17 => 34,
        18 => 36,
        19 => 38,
        20 => 42,
        21 => 50,
        _ => return Err(LzxError::BadWindow),
    })
}

fn slot_tables(slots: usize) -> (Vec<u32>, Vec<u32>) {
    let (mut extra, mut base) = (vec![0u32; slots], vec![0u32; slots]);
    for i in 0..slots {
        extra[i] = if i < 4 { 0 } else { (((i - 2) / 2) as u32).min(17) };
        if i > 0 {
            base[i] = base[i - 1] + (1 << extra[i - 1]);
        }
    }
    (extra, base)
}

/// Read delta-coded code lengths for `lens[start..end]` (previous values are the base for the deltas).
fn read_lengths(b: &mut Bits, lens: &mut [u8], start: usize, end: usize) -> Result<()> {
    let mut pre = [0u8; PRE_SYMS];
    for p in pre.iter_mut() {
        *p = b.take(4) as u8;
    }
    let pt = Huff::new(&pre)?;
    let mut i = start;
    while i < end {
        let sym = pt.decode(b)?;
        match sym {
            0..=16 => {
                lens[i] = ((lens[i] as i32 - sym as i32 + 17) % 17) as u8;
                i += 1;
            }
            17 | 18 => {
                let n = if sym == 17 { 4 + b.take(4) } else { 20 + b.take(5) } as usize;
                for _ in 0..n {
                    if i >= end {
                        return Err(LzxError::BadHuffman);
                    }
                    lens[i] = 0;
                    i += 1;
                }
            }
            _ => {
                // Run of 4..5 entries that all get the SAME new length, derived from the first entry's previous length.
                let n = 4 + b.take(1) as usize;
                let d = pt.decode(b)?;
                if d > 16 {
                    return Err(LzxError::BadHuffman);
                }
                let v = ((lens[i] as i32 - d as i32 + 17) % 17) as u8;
                for _ in 0..n {
                    if i >= end {
                        return Err(LzxError::BadHuffman);
                    }
                    lens[i] = v;
                    i += 1;
                }
            }
        }
        if b.overrun() {
            return Err(LzxError::Truncated);
        }
    }
    Ok(())
}

/// Decompress one LZX stream (one WIM chunk) that expands to exactly `out_len` bytes; `chunk_size` is the WIM chunk size.
pub fn decompress(src: &[u8], out_len: usize, chunk_size: u32) -> Result<Vec<u8>> {
    if !chunk_size.is_power_of_two() {
        return Err(LzxError::BadWindow);
    }
    let order = chunk_size.trailing_zeros();
    let slots = position_slots(order)?;
    let (extra, base) = slot_tables(slots);
    let num_main = NUM_CHARS + slots * 8;

    let mut b = Bits::new(src);
    let mut out: Vec<u8> = Vec::with_capacity(out_len);
    let (mut main_len, mut len_len) = (vec![0u8; num_main], vec![0u8; NUM_LEN_SYMS]);
    let mut rep = [1usize, 1, 1];

    while out.len() < out_len {
        let kind = b.take(3);
        let size = if b.take(1) == 1 {
            1usize << 15
        } else {
            let hi = b.take(16) as usize;
            if order >= 16 { hi << 8 | b.take(8) as usize } else { hi }
        };
        if size == 0 {
            return Err(LzxError::BadBlock);
        }
        let end = out.len() + size;
        if end > out_len {
            return Err(LzxError::BadBlock);
        }
        match kind {
            1 | 2 => {
                let mut aligned_len = [0u8; NUM_ALIGNED];
                if kind == 2 {
                    for a in aligned_len.iter_mut() {
                        *a = b.take(3) as u8;
                    }
                }
                let aligned = if kind == 2 { Some(Huff::new(&aligned_len)?) } else { None };
                read_lengths(&mut b, &mut main_len, 0, NUM_CHARS)?;
                read_lengths(&mut b, &mut main_len, NUM_CHARS, num_main)?;
                read_lengths(&mut b, &mut len_len, 0, NUM_LEN_SYMS)?;
                let (main, lent) = (Huff::new(&main_len)?, Huff::new(&len_len)?);

                while out.len() < end {
                    if b.overrun() {
                        return Err(LzxError::Truncated);
                    }
                    let sym = main.decode(&mut b)?;
                    if sym < NUM_CHARS {
                        out.push(sym as u8);
                        continue;
                    }
                    let m = sym - NUM_CHARS;
                    let (slot, lh) = (m >> 3, m & 7);
                    let len = if lh == 7 { MIN_MATCH + 7 + lent.decode(&mut b)? } else { MIN_MATCH + lh };
                    let off = match slot {
                        0 => rep[0],
                        1 => {
                            rep.swap(0, 1);
                            rep[0]
                        }
                        2 => {
                            rep.swap(0, 2);
                            rep[0]
                        }
                        _ => {
                            let eb = extra[slot];
                            let extra_val = if let (Some(al), true) = (&aligned, eb >= 3) {
                                let verbatim = b.take(eb - 3) << 3;
                                verbatim + al.decode(&mut b)? as u32
                            } else {
                                b.take(eb)
                            };
                            let o = (base[slot] + extra_val) as usize - 2;
                            rep[2] = rep[1];
                            rep[1] = rep[0];
                            rep[0] = o;
                            o
                        }
                    };
                    if off == 0 || off > out.len() || out.len() + len > end {
                        return Err(LzxError::BadMatch);
                    }
                    for _ in 0..len {
                        let c = out[out.len() - off];
                        out.push(c);
                    }
                }
            }
            3 => {
                // Uncompressed block: pad to a 16-bit boundary (1..16 bits), then R0..R2, the bytes, and a pad byte if odd.
                let pad = 16 - (b.consumed % 16) as u32;
                b.take(pad);
                // Whole words already loaded into the buffer are still ahead of `pos`; rewind to the byte position.
                let byte_pos = (b.consumed / 8) as usize;
                let raw = src.get(byte_pos..byte_pos + 12 + size).ok_or(LzxError::Truncated)?;
                for (i, r) in rep.iter_mut().enumerate() {
                    *r = u32::from_le_bytes([raw[4 * i], raw[4 * i + 1], raw[4 * i + 2], raw[4 * i + 3]]) as usize;
                }
                out.extend_from_slice(&raw[12..12 + size]);
                let consumed_bytes = 12 + size + (size & 1);
                b.buf = 0;
                b.n = 0;
                b.pos = byte_pos + consumed_bytes;
                b.consumed = (b.pos as u64) * 8;
            }
            _ => return Err(LzxError::BadBlock),
        }
    }
    e8_undo(&mut out);
    Ok(out)
}

/// Undo the E8 (x86 CALL) address translation applied by the compressor.
fn e8_undo(d: &mut [u8]) {
    if d.len() <= 10 {
        return;
    }
    let mut i = 0usize;
    while i < d.len() - 10 {
        if d[i] != 0xE8 {
            i += 1;
            continue;
        }
        let abs = i32::from_le_bytes([d[i + 1], d[i + 2], d[i + 3], d[i + 4]]) as i64;
        if abs >= -(i as i64) && abs < WIM_E8_FILE_SIZE {
            let rel = if abs >= 0 { abs - i as i64 } else { abs + WIM_E8_FILE_SIZE };
            d[i + 1..i + 5].copy_from_slice(&(rel as i32).to_le_bytes());
        }
        i += 5;
    }
}
