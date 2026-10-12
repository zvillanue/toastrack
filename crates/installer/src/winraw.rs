//! Raw disk access on Windows: finding physical drives, taking their volumes offline, and a `Device` that
//! accepts the byte-granular reads and writes the installer makes.
//!
//! Not exercised by automated tests (they need a real disk); the sector-alignment logic it relies on is in
//! [`crate::sectorio`] and is tested everywhere.

#![cfg(windows)]

use crate::{sectorio::SectorIo, Error, Result};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::windows::{
        ffi::OsStrExt,
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    ptr,
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{CreateFileW, FlushFileBuffers, GetLogicalDrives, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING},
    System::{
        Ioctl::{
            DISK_GEOMETRY_EX, FSCTL_ALLOW_EXTENDED_DASD_IO, FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, GET_LENGTH_INFORMATION,
            IOCTL_DISK_DELETE_DRIVE_LAYOUT, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_DISK_GET_LENGTH_INFO,
            IOCTL_DISK_UPDATE_PROPERTIES, IOCTL_STORAGE_GET_DEVICE_NUMBER, IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_DEVICE_DESCRIPTOR,
            STORAGE_DEVICE_NUMBER, STORAGE_PROPERTY_QUERY,
        },
        IO::DeviceIoControl,
    },
};

const BUS_TYPE_USB: i32 = 7;
const FILE_DEVICE_DISK: u32 = 7;
/// Highest physical drive number probed when listing.
const MAX_DRIVES: u32 = 32;

#[derive(Debug, Clone)]
pub struct RawInfo {
    pub index: u32,
    pub size: u64,
    pub sector: u32,
    pub usb: bool,
    pub removable: bool,
    pub model: String,
    /// Drive letters of the volumes on this disk.
    pub letters: Vec<char>,
}

pub fn path_for(index: u32) -> String {
    format!(r"\\.\PhysicalDrive{index}")
}

/// `\\.\PhysicalDriveN` (any case) to N.
pub fn parse_index(path: &str) -> Option<u32> {
    let p = path.to_ascii_lowercase();
    p.strip_prefix(r"\\.\physicaldrive")?.parse().ok()
}

fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

/// Open a device path with the given access; sharing is permissive so that other users of the disk are not locked out.
fn open_raw(path: &str, access: u32) -> io::Result<OwnedHandle> {
    let w = wide(path);
    // SAFETY: `w` is a NUL-terminated UTF-16 string that outlives the call; the other arguments are plain values or null.
    let h = unsafe { CreateFileW(w.as_ptr(), access, FILE_SHARE_READ | FILE_SHARE_WRITE, ptr::null(), OPEN_EXISTING, 0, ptr::null_mut()) };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateFileW returned a valid handle that nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(h as _) })
}

/// DeviceIoControl with an optional input and a caller-sized output; returns the number of output bytes.
fn ioctl(h: HANDLE, code: u32, input: Option<&[u8]>, out: &mut [u8]) -> io::Result<usize> {
    let mut returned = 0u32;
    let (ip, il) = input.map_or((ptr::null(), 0), |b| (b.as_ptr().cast(), b.len() as u32));
    let (op, ol) = if out.is_empty() { (ptr::null_mut(), 0) } else { (out.as_mut_ptr().cast(), out.len() as u32) };
    // SAFETY: the pointers and lengths describe live buffers owned by the caller for the duration of the call.
    let ok = unsafe { DeviceIoControl(h, code, ip, il, op, ol, &mut returned, ptr::null_mut()) };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(returned as usize)
    }
}

fn raw(h: &OwnedHandle) -> HANDLE {
    h.as_raw_handle() as HANDLE
}

fn c_string_at(buf: &[u8], off: u32) -> String {
    if off == 0 || off as usize >= buf.len() {
        return String::new();
    }
    let tail = &buf[off as usize..];
    let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
    String::from_utf8_lossy(&tail[..end]).trim().to_string()
}

fn drive_letters() -> Vec<char> {
    // SAFETY: no arguments.
    let mask = unsafe { GetLogicalDrives() };
    (0..26).filter(|i| mask & (1 << i) != 0).map(|i| (b'A' + i as u8) as char).collect()
}

/// Which physical disk (and partition) a lettered volume lives on.
fn disk_of_letter(letter: char) -> Option<u32> {
    let h = open_raw(&format!(r"\\.\{letter}:"), 0).ok()?;
    let mut out = [0u8; std::mem::size_of::<STORAGE_DEVICE_NUMBER>()];
    ioctl(raw(&h), IOCTL_STORAGE_GET_DEVICE_NUMBER, None, &mut out).ok()?;
    // SAFETY: the buffer is exactly one STORAGE_DEVICE_NUMBER (plain integers) filled by the driver.
    let n: STORAGE_DEVICE_NUMBER = unsafe { ptr::read_unaligned(out.as_ptr().cast()) };
    (n.DeviceType == FILE_DEVICE_DISK).then_some(n.DeviceNumber)
}

/// Properties of physical drive `index`, or None if there is no such drive.
pub fn query(index: u32) -> Option<RawInfo> {
    let h = open_raw(&path_for(index), 0).ok()?;
    let hd = raw(&h);

    let mut len = [0u8; std::mem::size_of::<GET_LENGTH_INFORMATION>()];
    ioctl(hd, IOCTL_DISK_GET_LENGTH_INFO, None, &mut len).ok()?;
    let size = u64::from_le_bytes(len[..8].try_into().ok()?);

    let mut geo = vec![0u8; std::mem::size_of::<DISK_GEOMETRY_EX>() + 256];
    let sector = match ioctl(hd, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, None, &mut geo) {
        // DISK_GEOMETRY_EX starts with DISK_GEOMETRY: cylinders(8), media(4), tracks(4), sectors(4), bytes per sector(4)
        Ok(_) => u32::from_le_bytes(geo[20..24].try_into().ok()?),
        Err(_) => 512,
    };

    // STORAGE_PROPERTY_QUERY { PropertyId: StorageDeviceProperty (0), QueryType: PropertyStandardQuery (0), ... }
    let query = [0u8; std::mem::size_of::<STORAGE_PROPERTY_QUERY>()];
    let mut desc = vec![0u8; 1024];
    let (mut usb, mut removable, mut model) = (false, false, String::new());
    if ioctl(hd, IOCTL_STORAGE_QUERY_PROPERTY, Some(&query), &mut desc).is_ok() {
        // SAFETY: the buffer starts with a STORAGE_DEVICE_DESCRIPTOR (plain integers) filled by the driver.
        let d: STORAGE_DEVICE_DESCRIPTOR = unsafe { ptr::read_unaligned(desc.as_ptr().cast()) };
        usb = d.BusType == BUS_TYPE_USB;
        removable = d.RemovableMedia != 0;
        let vendor = c_string_at(&desc, d.VendorIdOffset);
        let product = c_string_at(&desc, d.ProductIdOffset);
        model = format!("{vendor} {product}").trim().to_string();
    }

    let letters = drive_letters().into_iter().filter(|c| disk_of_letter(*c) == Some(index)).collect();
    Some(RawInfo { index, size, sector, usb, removable, model, letters })
}

pub fn list() -> Vec<RawInfo> {
    (0..MAX_DRIVES).filter_map(query).filter(|d| d.size > 0).collect()
}

/// The disk holding the running Windows (so it is never offered as a target).
pub fn system_disk() -> Option<u32> {
    let drive = std::env::var("SystemDrive").ok()?;
    disk_of_letter(drive.chars().next()?.to_ascii_uppercase())
}

/// Volumes held locked and dismounted while the disk is written, so that Windows does not write to them or re-mount them.
#[allow(dead_code)] // the handles are held only so the locks stay in force
struct Locks(Vec<OwnedHandle>);

fn lock_volumes(letters: &[char]) -> Result<Locks> {
    let mut held = Vec::new();
    for l in letters {
        let h = open_raw(&format!(r"\\.\{l}:"), GENERIC_READ | GENERIC_WRITE)
            .map_err(|e| Error::Unsafe(format!("cannot open volume {l}: ({e}); run as administrator")))?;
        // Programs that still have files open on the volume make locking fail for a while; retry like disk tools do.
        let mut locked = false;
        for _ in 0..20 {
            if ioctl(raw(&h), FSCTL_LOCK_VOLUME, None, &mut []).is_ok() {
                locked = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        if !locked {
            return Err(Error::Unsafe(format!("volume {l}: is in use. Close File Explorer windows and programs using the stick, then try again")));
        }
        let _ = ioctl(raw(&h), FSCTL_DISMOUNT_VOLUME, None, &mut []);
        held.push(h);
    }
    Ok(Locks(held))
}

/// A physical drive opened for byte-granular access. Dropping it releases the volume locks.
pub struct WinDisk {
    io: SectorIo<File>,
    writable: bool,
    _locks: Locks,
}

impl std::fmt::Debug for WinDisk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WinDisk {{ writable: {} }}", self.writable)
    }
}

/// Open drive `index`. For writing, its volumes are locked and dismounted and the old partition table is dropped first.
pub fn open(index: u32, writable: bool) -> Result<WinDisk> {
    let info = query(index).ok_or_else(|| Error::Unsafe(format!("no physical drive {index}")))?;
    if info.sector != 512 {
        return Err(Error::Unsafe(format!("drive {index} has {}-byte sectors; only 512-byte sectors are supported so far", info.sector)));
    }
    let locks = if writable { lock_volumes(&info.letters)? } else { Locks(Vec::new()) };
    let file = OpenOptions::new()
        .read(true)
        .write(writable)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE) as u32)
        .open(path_for(index))
        .map_err(|e| Error::Unsafe(format!("cannot open {}: {e} (run as administrator)", path_for(index))))?;
    if writable {
        let h = file.as_raw_handle() as HANDLE;
        let _ = ioctl(h, FSCTL_ALLOW_EXTENDED_DASD_IO, None, &mut []);
        let _ = ioctl(h, IOCTL_DISK_DELETE_DRIVE_LAYOUT, None, &mut []);
    }
    Ok(WinDisk { io: SectorIo::new(file, info.size, info.sector as u64), writable, _locks: locks })
}

impl WinDisk {
    /// Flush to the device and make Windows re-read the new partition table.
    pub fn finish(mut self) -> io::Result<()> {
        self.io.flush()?;
        let h = self.io.get_mut().as_raw_handle() as HANDLE;
        // SAFETY: `h` is the open disk handle.
        unsafe { FlushFileBuffers(h) };
        if self.writable {
            let _ = ioctl(h, IOCTL_DISK_UPDATE_PROPERTIES, None, &mut []);
        }
        Ok(())
    }
}

impl Read for WinDisk {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        self.io.read(b)
    }
}
impl Write for WinDisk {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.io.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.io.flush()
    }
}
impl Seek for WinDisk {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.io.seek(to)
    }
}
