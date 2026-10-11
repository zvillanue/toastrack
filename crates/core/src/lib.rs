//! Toastrack core: figure out whether an ISO can boot under UEFI Secure Boot.

pub mod iso;
pub mod pe;
pub mod verdict;

pub use verdict::{analyze_iso, SecureBootStatus};
