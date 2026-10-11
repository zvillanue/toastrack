//! Create, update and verify Toastrack USB sticks.
//!
//! Works on a block device or a plain image file, so everything is testable without hardware.

pub mod bundle;
pub mod device;
pub mod exfat_format;
pub mod install;
pub mod layout;

use std::fmt;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Gpt(String),
    Fat(String),
    Bundle(String),
    Layout(String),
    Unsafe(String),
    Verify(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Gpt(e) => write!(f, "partition table error: {e}"),
            Error::Fat(e) => write!(f, "filesystem error: {e}"),
            Error::Bundle(e) => write!(f, "bad bundle: {e}"),
            Error::Layout(e) => write!(f, "{e}"),
            Error::Unsafe(e) => write!(f, "refusing to continue: {e}"),
            Error::Verify(e) => write!(f, "verification failed: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
