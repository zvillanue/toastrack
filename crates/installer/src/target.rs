//! Opening the thing `install`/`update`/`verify` write to: an image file, a Linux block device, or a Windows physical drive.

use crate::{device, Error, Result};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
};

#[derive(Debug)]
pub enum Target {
    File(File),
    #[cfg(windows)]
    Disk(crate::winraw::WinDisk),
}

impl Target {
    /// Open `path` for the installer. Image files may be created and sized; physical drives never are.
    /// `writable` false opens for reading only (no volumes are locked).
    pub fn open(path: &Path, writable: bool, size: Option<u64>, create: bool) -> Result<Target> {
        #[cfg(windows)]
        if let Some(idx) = crate::winraw::parse_index(&path.to_string_lossy()) {
            return Ok(Target::Disk(crate::winraw::open(idx, writable)?));
        }
        let mut o = OpenOptions::new();
        o.read(true).write(writable);
        if create {
            o.create(true);
        }
        let f = o.open(path)?;
        if let Some(sz) = size {
            if !device::is_block_device(path) {
                f.set_len(sz)?;
            }
        }
        Ok(Target::File(f))
    }

    /// Flush everything to the device (and, on Windows, make the system re-read the new partition table).
    pub fn finish(self) -> Result<()> {
        match self {
            Target::File(f) => f.sync_all().map_err(Error::from),
            #[cfg(windows)]
            Target::Disk(d) => d.finish().map_err(Error::from),
        }
    }
}

impl Read for Target {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        match self {
            Target::File(f) => f.read(b),
            #[cfg(windows)]
            Target::Disk(d) => d.read(b),
        }
    }
}
impl Write for Target {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        match self {
            Target::File(f) => f.write(b),
            #[cfg(windows)]
            Target::Disk(d) => d.write(b),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Target::File(f) => f.flush(),
            #[cfg(windows)]
            Target::Disk(d) => d.flush(),
        }
    }
}
impl Seek for Target {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        match self {
            Target::File(f) => f.seek(to),
            #[cfg(windows)]
            Target::Disk(d) => d.seek(to),
        }
    }
}
