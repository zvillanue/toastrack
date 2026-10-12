//! The hand-off from the loader to the Windows helper (`toastrack-attach.exe`).
//!
//! Windows Setup cannot see the virtual disk the firmware used to start the ISO, so it finds no `install.wim`.
//! The loader therefore leaves a UEFI variable naming the ISO (volume serial + path on the stick); a helper started
//! by Setup's `autounattend.xml` reads it, finds the volume, and attaches the ISO as a virtual DVD with the Windows
//! virtual-disk API, after which Setup finds `sources\install.wim` on that DVD.

use alloc::{format, string::String, vec::Vec};

pub const VAR_NAME: &str = "ToastrackIso";
/// Vendor GUID of the variable (also used by the helper, as text).
pub const VAR_GUID: &str = "5f0b6a2e-3c1d-4b8e-9a77-7a6f1d0c2e42";
const MAGIC: &str = "TOASTRACK1";
const MAX_PATH: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Volume serial number of the data partition, as Windows reports it (the `XXXX-XXXX` of exFAT/FAT).
    pub serial: u32,
    /// Path of the ISO on that volume, `/`-separated, starting with `/`.
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestError {
    Format,
    Path,
}

fn path_ok(p: &str) -> bool {
    p.starts_with('/')
        && p.len() <= MAX_PATH
        && !p.chars().any(|c| c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        && !p.split('/').any(|s| s == "..")
}

impl Request {
    pub fn new(serial: u32, path: &str) -> Result<Self, RequestError> {
        if path_ok(path) { Ok(Self { serial, path: path.into() }) } else { Err(RequestError::Path) }
    }

    pub fn encode(&self) -> Vec<u8> {
        format!("{MAGIC}\nserial={:08X}\npath={}\n", self.serial, self.path).into_bytes()
    }

    pub fn decode(b: &[u8]) -> Result<Self, RequestError> {
        let t = core::str::from_utf8(b).map_err(|_| RequestError::Format)?;
        let mut lines = t.lines();
        if lines.next() != Some(MAGIC) {
            return Err(RequestError::Format);
        }
        let (mut serial, mut path) = (None, None);
        for l in lines {
            if let Some(v) = l.strip_prefix("serial=") {
                serial = u32::from_str_radix(v, 16).ok();
            } else if let Some(v) = l.strip_prefix("path=") {
                path = Some(v);
            }
        }
        Request::new(serial.ok_or(RequestError::Format)?, path.ok_or(RequestError::Format)?)
    }

    /// The ISO's path as Windows spells it on `drive`, e.g. `E:\isos\Win11.iso`.
    pub fn windows_path(&self, drive: char) -> String {
        format!("{}:{}", drive.to_ascii_uppercase(), self.path.replace('/', "\\"))
    }
}

/// The drive letter of the volume with this serial number (first match in letter order).
pub fn pick_drive(volumes: &[(char, u32)], serial: u32) -> Option<char> {
    let mut v: Vec<_> = volumes.iter().filter(|(_, s)| *s == serial).map(|(d, _)| *d).collect();
    v.sort();
    v.first().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_windows_path() {
        let r = Request::new(0xCAFE_F00D, "/isos/Win11 25H2.iso").unwrap();
        let back = Request::decode(&r.encode()).unwrap();
        assert_eq!(back, r);
        assert_eq!(back.windows_path('e'), "E:\\isos\\Win11 25H2.iso");
    }

    #[test]
    fn rejects_malformed_and_dangerous_requests() {
        assert!(Request::decode(b"").is_err());
        assert!(Request::decode(b"WRONG\nserial=1\npath=/a.iso\n").is_err());
        assert!(Request::decode(b"TOASTRACK1\nserial=zz\npath=/a.iso\n").is_err());
        assert!(Request::decode(b"TOASTRACK1\nserial=1\n").is_err());
        for bad in ["a.iso", "/../x.iso", "/a/../../b.iso", "/a\\b.iso", "C:/x.iso", "/a\nb.iso", "/a|b.iso"] {
            assert!(Request::new(1, bad).is_err(), "{bad}");
        }
        assert!(Request::new(1, &alloc::format!("/{}", "x".repeat(2000))).is_err());
    }

    #[test]
    fn picks_the_matching_volume_in_letter_order() {
        let v = [('C', 0x1111), ('F', 0xCAFE), ('D', 0xCAFE), ('E', 0x2222)];
        assert_eq!(pick_drive(&v, 0xCAFE), Some('D'));
        assert_eq!(pick_drive(&v, 0x9999), None);
    }
}
