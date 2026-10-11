//! Decide whether an ISO's EFI loader will pass Secure Boot.

use crate::{fat::FatImage, iso::{Iso, IsoError}, pe};
use crate::io::ReadAt;
use alloc::{string::String, vec::Vec};

/// Where an ISO's firmware-visible loader is expected.
const LOADER_PATH: &str = "EFI/BOOT/BOOTX64.EFI";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_and_uefi_ca_signers_are_trusted_others_are_not() {
        let info = |s: &str| pe::PeInfo { machine: pe::MACHINE_X64, authenticode_sha256: [0; 32], signer_subjects: alloc::vec![s.into()], signed: true, sections: Vec::new() };
        for ok in [
            "CN=Microsoft Windows Production PCA 2011,O=Microsoft Corporation",
            "CN=Microsoft Corporation UEFI CA 2011,O=Microsoft Corporation",
            "CN=Windows UEFI CA 2023,O=Microsoft Corporation",
        ] {
            assert!(classify(&info(ok)).boots_with_secure_boot(), "{ok}");
        }
        assert!(!classify(&info("CN=Canonical Ltd. Master Certificate Authority")).boots_with_secure_boot());
        assert!(classify(&info("CN=Microsoft Windows Production PCA 2011")).is_windows());
        assert!(!classify(&info("CN=Microsoft Corporation UEFI CA 2011")).is_windows());
    }
}

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
    /// The loader is the Windows boot manager (a Windows installer or Windows PE image).
    pub fn is_windows(&self) -> bool {
        match self {
            Self::Trusted { signers } | Self::NeedsEnrollment { signers } => is_windows_signer(signers),
            _ => false,
        }
    }

    pub fn boots_with_secure_boot(&self) -> bool {
        matches!(self, Self::Trusted { .. })
    }
}

/// Roots every stock PC's firmware trusts: the UEFI CA (shims, option ROMs) and the Windows CAs
/// (the Windows boot manager). "Windows UEFI CA 2023" is only trusted on firmware that received the 2023 db update.
fn is_microsoft(subject: &str) -> bool {
    subject.contains("Microsoft Corporation UEFI CA")
        || subject.contains("Microsoft UEFI CA")
        || subject.contains("Microsoft Windows Production PCA")
        || subject.contains("Windows UEFI CA")
}

/// True if the signature chain is Microsoft's *Windows* signing (the Windows boot manager), as opposed to a
/// third-party loader signed through the UEFI CA.
pub fn is_windows_signer(signers: &[String]) -> bool {
    signers.iter().any(|s| s.contains("Microsoft Windows") || s.contains("Windows UEFI CA"))
}

pub fn analyze_iso<R: ReadAt>(iso: &mut Iso<R>) -> Result<SecureBootStatus, IsoError> {
    let data = match iso.find(LOADER_PATH)? {
        Some(entry) => iso.read_file(&entry)?,
        // Hybrid ISOs often keep the loader only inside the El Torito EFI image (a FAT filesystem).
        None => match loader_from_efi_image(iso)? {
            Some(d) => d,
            None => return Ok(SecureBootStatus::NoUefiLoader),
        },
    };
    let info = match pe::inspect(&data) {
        Ok(i) if i.machine == pe::MACHINE_X64 => i,
        _ => return Ok(SecureBootStatus::NoUefiLoader),
    };
    Ok(classify(&info))
}

/// Classify a parsed PE image for Secure Boot purposes.
pub fn classify(info: &pe::PeInfo) -> SecureBootStatus {
    if !info.signed {
        SecureBootStatus::Unsigned
    } else if info.signer_subjects.iter().any(|s| is_microsoft(s)) {
        SecureBootStatus::Trusted { signers: info.signer_subjects.clone() }
    } else {
        SecureBootStatus::NeedsEnrollment { signers: info.signer_subjects.clone() }
    }
}

/// A window onto the ISO starting at `off`, for reading a FAT image embedded in it.
struct IsoWindow<'a, R> {
    iso: &'a mut Iso<R>,
    off: u64,
}

impl<R: ReadAt> ReadAt for IsoWindow<'_, R> {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), crate::io::IoError> {
        self.iso.read_raw(self.off + offset, buf)
    }
}

fn loader_from_efi_image<R: ReadAt>(iso: &mut Iso<R>) -> Result<Option<Vec<u8>>, IsoError> {
    for off in iso.efi_boot_images()? {
        let Ok(mut fat) = FatImage::open(IsoWindow { iso, off }) else { continue };
        if let Ok(Some(e)) = fat.find(LOADER_PATH) {
            if let Ok(d) = fat.read_file(&e) {
                return Ok(Some(d));
            }
        }
    }
    Ok(None)
}
