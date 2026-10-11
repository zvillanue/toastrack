//! Toastrack core: figure out whether an ISO can boot under UEFI Secure Boot.
//!
//! `no_std` + `alloc` so the same code runs inside the UEFI loader and in host tools.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod bootcfg;
pub mod exfat;
pub mod fat;
pub mod io;
pub mod iso;
pub mod profile;
pub mod wim;
pub mod pe;
pub mod udf;
pub mod verdict;

pub use io::{IoError, ReadAt};
pub use verdict::{analyze_iso, SecureBootStatus};
