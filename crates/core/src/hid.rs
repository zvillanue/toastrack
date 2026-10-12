//! Just enough of USB HID report descriptors to drive a mouse or tablet ourselves, for firmware that has no pointer driver.

use alloc::vec::Vec;

/// A bit field inside an input report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    pub bit: u32,
    pub size: u32,
    pub min: i32,
    pub max: i32,
}

impl Field {
    fn read(&self, report: &[u8], base_bit: u32) -> Option<i32> {
        if self.size == 0 || self.size > 32 {
            return None;
        }
        let mut v: u64 = 0;
        for i in 0..self.size {
            let b = base_bit + self.bit + i;
            let byte = *report.get((b / 8) as usize)?;
            v |= (((byte >> (b % 8)) & 1) as u64) << i;
        }
        let v = v as i64;
        // Sign-extend when the field's logical range is negative.
        Some(if self.min < 0 && self.size < 32 && v & (1 << (self.size - 1)) != 0 { (v - (1 << self.size)) as i32 } else { v as i32 })
    }
}

/// Where a pointing device keeps its X, Y and left button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MouseLayout {
    /// 0 when the device does not use report IDs; otherwise the first byte of the report.
    pub report_id: u8,
    pub x: Field,
    pub y: Field,
    pub left: Option<Field>,
    /// True for tablets (positions); false for mice (movements).
    pub absolute: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub x: i32,
    pub y: i32,
    pub left: bool,
}

impl MouseLayout {
    pub fn decode(&self, report: &[u8]) -> Option<Sample> {
        let base = if self.report_id != 0 {
            if *report.first()? != self.report_id {
                return None;
            }
            8
        } else {
            0
        };
        Some(Sample {
            x: self.x.read(report, base)?,
            y: self.y.read(report, base)?,
            left: self.left.and_then(|f| f.read(report, base)).is_some_and(|v| v != 0),
        })
    }
}

/// Find the pointer's X/Y/button fields in a report descriptor; None if it describes something else (a keyboard).
pub fn parse_mouse(desc: &[u8]) -> Option<MouseLayout> {
    let (mut page, mut lmin, mut lmax, mut rsize, mut rcount, mut rid) = (0u32, 0i32, 0i32, 0u32, 0u32, 0u8);
    let mut usages: Vec<u32> = Vec::new();
    let (mut umin, mut umax) = (None::<u32>, None::<u32>);
    let mut offsets: Vec<(u8, u32)> = Vec::new();
    let (mut x, mut y, mut left): (Option<(u8, Field, bool)>, Option<(u8, Field)>, Option<(u8, Field)>) = (None, None, None);

    let mut i = 0;
    while i < desc.len() {
        let b = desc[i];
        if b == 0xFE {
            i += 3 + *desc.get(i + 1)? as usize; // long item: skip
            continue;
        }
        let size = match b & 3 { 3 => 4, n => n as usize };
        let (kind, tag) = ((b >> 2) & 3, b >> 4);
        let data = desc.get(i + 1..i + 1 + size)?;
        let (mut u, mut s) = (0u32, 0i32);
        for (k, d) in data.iter().enumerate() {
            u |= (*d as u32) << (8 * k);
        }
        if size > 0 {
            let shift = 32 - 8 * size as u32;
            s = ((u << shift) as i32) >> shift;
        }
        match (kind, tag) {
            (1, 0) => page = u,
            (1, 1) => lmin = s,
            (1, 2) => lmax = if lmin < 0 { s } else { u as i32 },
            (1, 7) => rsize = u,
            (1, 8) => rid = u as u8,
            (1, 9) => rcount = u,
            (2, 0) => usages.push(if size == 4 { u } else { (page << 16) | u }),
            (2, 1) => umin = Some(if size == 4 { u } else { (page << 16) | u }),
            (2, 2) => umax = Some(if size == 4 { u } else { (page << 16) | u }),
            (0, 8) => {
                // Input item.
                let slot = match offsets.iter().position(|(id, _)| *id == rid) {
                    Some(p) => p,
                    None => {
                        offsets.push((rid, 0));
                        offsets.len() - 1
                    }
                };
                let constant = u & 1 != 0;
                let variable = u & 2 != 0;
                if !constant && variable {
                    for n in 0..rcount {
                        let usage = usages.get(n as usize).copied().or_else(|| usages.last().copied()).or_else(|| umin.map(|m| m + n));
                        let usage = match (usages.get(n as usize), umin, umax) {
                            (None, Some(lo), Some(hi)) if usages.is_empty() => Some((lo + n).min(hi)),
                            _ => usage,
                        };
                        let field = Field { bit: offsets[slot].1 + n * rsize, size: rsize, min: lmin, max: lmax };
                        match usage {
                            Some(0x0001_0030) if x.is_none() => x = Some((rid, field, u & 4 == 0)),
                            Some(0x0001_0031) if y.is_none() => y = Some((rid, field)),
                            Some(0x0009_0001) if left.is_none() => left = Some((rid, field)),
                            _ => {}
                        }
                    }
                }
                offsets[slot].1 += rcount * rsize;
                usages.clear();
                umin = None;
                umax = None;
            }
            (0, _) => {
                usages.clear();
                umin = None;
                umax = None;
            }
            _ => {}
        }
        i += 1 + size;
    }
    let ((xid, xf, absolute), (yid, yf)) = (x?, y?);
    if xid != yid {
        return None;
    }
    Some(MouseLayout { report_id: xid, x: xf, y: yf, left: left.filter(|(id, _)| *id == xid).map(|(_, f)| f), absolute })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The standard boot-protocol mouse: 3 buttons, 8-bit relative X/Y, wheel.
    const MOUSE: [u8; 52] = [
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x03, 0x15, 0x00, 0x25, 0x01, 0x95,
        0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05, 0x81, 0x03, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x09, 0x38, 0x15, 0x81,
        0x25, 0x7F, 0x75, 0x08, 0x95, 0x03, 0x81, 0x06, 0xC0, 0xC0,
    ];
    /// A tablet: 3 buttons, 16-bit absolute X/Y (0..32767), wheel.
    const TABLET: [u8; 76] = [
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x03, 0x15, 0x00, 0x25, 0x01, 0x95,
        0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05, 0x81, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x00, 0x26, 0xFF,
        0x7F, 0x35, 0x00, 0x46, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x02, 0x81, 0x02, 0x05, 0x01, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7F, 0x35,
        0x00, 0x45, 0x00, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06, 0xC0, 0xC0, 0x00, 0x00,
    ];
    /// A boot keyboard must not be taken for a mouse.
    const KEYBOARD: [u8; 24] = [
        0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81,
        0x02, 0xC0, 0x00,
    ];

    #[test]
    fn boot_mouse() {
        let l = parse_mouse(&MOUSE).unwrap();
        assert!(!l.absolute && l.report_id == 0);
        // buttons=1 (left), dx=5, dy=-3, wheel=0
        assert_eq!(l.decode(&[0x01, 5, 0xFD, 0]), Some(Sample { x: 5, y: -3, left: true }));
        assert_eq!(l.decode(&[0x00, 0x80, 0x7F, 0]), Some(Sample { x: -128, y: 127, left: false }));
    }

    #[test]
    fn tablet() {
        let l = parse_mouse(&TABLET).unwrap();
        assert!(l.absolute);
        assert_eq!((l.x.max, l.y.max), (32767, 32767));
        // buttons=1, x=0x1234, y=0x7FFF, wheel=0
        assert_eq!(l.decode(&[0x01, 0x34, 0x12, 0xFF, 0x7F, 0]), Some(Sample { x: 0x1234, y: 0x7FFF, left: true }));
    }

    #[test]
    fn keyboard_is_not_a_mouse() {
        assert_eq!(parse_mouse(&KEYBOARD), None);
        assert_eq!(parse_mouse(&[]), None);
        assert_eq!(parse_mouse(&[0xA1]), None);
    }

    #[test]
    fn report_ids_prefix_the_report() {
        // Same mouse but with report id 2.
        let mut d = alloc::vec![0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x02];
        d.extend_from_slice(&MOUSE[6..]);
        let l = parse_mouse(&d).unwrap();
        assert_eq!(l.report_id, 2);
        assert_eq!(l.decode(&[2, 0x01, 7, 1, 0]), Some(Sample { x: 7, y: 1, left: true }));
        assert_eq!(l.decode(&[1, 0x01, 7, 1, 0]), None);
    }
}
