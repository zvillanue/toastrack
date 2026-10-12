//! Minimal positional-read abstraction usable without `std`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoError;

impl core::fmt::Display for IoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("read error")
    }
}

pub trait ReadAt {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), IoError>;
}

#[cfg(feature = "std")]
impl<R: std::io::Read + std::io::Seek> ReadAt for R {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), IoError> {
        self.seek(std::io::SeekFrom::Start(offset)).map_err(|_| IoError)?;
        self.read_exact(buf).map_err(|_| IoError)
    }
}

use alloc::rc::Rc;
use core::cell::RefCell;

/// Share one reader between several consumers (e.g. a volume and the files opened from it).
pub struct Shared<R>(Rc<RefCell<R>>);

impl<R> Shared<R> {
    pub fn new(r: R) -> Self {
        Self(Rc::new(RefCell::new(r)))
    }
}

impl<R> Clone for Shared<R> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<R: ReadAt> ReadAt for Shared<R> {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), IoError> {
        self.0.borrow_mut().read_at(offset, buf)
    }
}

/// A byte range of another reader, addressed from 0 (e.g. one partition of a disk).
#[derive(Clone)]
pub struct Window<R> {
    pub inner: R,
    pub offset: u64,
    pub len: u64,
}

impl<R: ReadAt> ReadAt for Window<R> {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), IoError> {
        if offset.checked_add(buf.len() as u64).is_none_or(|end| end > self.len) {
            return Err(IoError);
        }
        self.inner.read_at(self.offset + offset, buf)
    }
}
