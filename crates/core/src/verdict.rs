//! Decide whether an ISO's EFI loader will pass Secure Boot.

use crate::{iso::Iso, pe};
use std::io::{Read, Seek};

/// Where an ISO's firmware-visible loader is expected.
const LOADER_PATH: &str = "EFI/BOOT/BOOTX64.EFI";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecureBootStatus {
    /// Signed by Microsoft's UEFI CA: boots on stock firmware (modulo dbx revocations).
    Trusted { signers: Vec<String> },
    /// Signed, but not by a Microsoft root (distro/vendor key). Boots only if that key is enrolled (MOK/db).
    NeedsEnrollment { signers: Vec<String> },
    /// EFI loader present but carries no signature: will not boot with Secure Boot on.
    Unsigned,
    /// No x86-64 UEFI loader at the standard path (BIOS-only, ARM, or loader inside El Torito image).
    NoUefiLoader,
}

impl SecureBootStatus {
    pub fn boots_with_secure_boot(&self) -> bool {
        matches!(self, Self::Trusted { .. })
    }
}

fn is_microsoft(subject: &str) -> bool {
    subject.contains("Microsoft Corporation UEFI CA") || subject.contains("Microsoft UEFI CA")
}

pub fn analyze_iso<R: Read + Seek>(iso: &mut Iso<R>) -> Result<SecureBootStatus, Box<dyn std::error::Error>> {
    let Some(entry) = iso.find(LOADER_PATH)? else {
        return Ok(SecureBootStatus::NoUefiLoader);
    };
    let data = iso.read_file(&entry)?;
    let info = match pe::inspect(&data) {
        Ok(i) if i.machine == pe::MACHINE_X64 => i,
        _ => return Ok(SecureBootStatus::NoUefiLoader),
    };
    if !info.signed {
        return Ok(SecureBootStatus::Unsigned);
    }
    if info.signer_subjects.iter().any(|s| is_microsoft(s)) {
        Ok(SecureBootStatus::Trusted { signers: info.signer_subjects })
    } else {
        Ok(SecureBootStatus::NeedsEnrollment { signers: info.signer_subjects })
    }
}
