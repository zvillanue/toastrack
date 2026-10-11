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
