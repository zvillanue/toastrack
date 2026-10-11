//! The files that go onto the stick's EFI partition, and checks that catch the mistakes that
//! otherwise only show up as a Secure Boot failure on the target PC.

use crate::{Error, Result};
use std::{fs, path::Path};
use toastrack_core::{
    pe::{self, PeInfo, MACHINE_X64},
    verdict::classify,
    SecureBootStatus,
};
use x509_cert::{certificate::Certificate, der::Decode};

pub const SHIM: &str = "shimx64.efi";
pub const MOK_MANAGER: &str = "mmx64.efi";
pub const LOADER: &str = "grubx64.efi";
pub const CERT: &str = "ENROLL_THIS_KEY_IN_MOKMANAGER.cer";

#[derive(Debug, Clone)]
pub struct Bundle {
    /// Microsoft-signed shim, started by the firmware as the default boot file.
    pub shim: Vec<u8>,
    /// MokManager, shipped alongside shim; used for the one-time key enrollment.
    pub mok_manager: Vec<u8>,
    /// Toastrack loader signed with the release key; shim starts it under this name.
    pub loader: Vec<u8>,
    /// The release signing certificate (DER), enrolled via MokManager on first boot.
    pub cert: Vec<u8>,
}

fn read(dir: &Path, name: &str) -> Result<Vec<u8>> {
    fs::read(dir.join(name)).map_err(|e| Error::Bundle(format!("{name}: {e}")))
}

fn parse(name: &str, data: &[u8]) -> Result<PeInfo> {
    let info = pe::inspect(data).map_err(|e| Error::Bundle(format!("{name}: not a valid PE image ({e:?})")))?;
    if info.machine != MACHINE_X64 {
        return Err(Error::Bundle(format!("{name}: not an x86-64 image")));
    }
    Ok(info)
}

impl Bundle {
    pub fn load(dir: &Path) -> Result<Self> {
        let b = Self {
            shim: read(dir, SHIM)?,
            mok_manager: read(dir, MOK_MANAGER)?,
            loader: read(dir, LOADER)?,
            cert: read(dir, CERT)?,
        };
        b.validate()?;
        Ok(b)
    }

    /// Checks that the pieces fit together for Secure Boot.
    pub fn validate(&self) -> Result<()> {
        let shim = parse(SHIM, &self.shim)?;
        if !matches!(classify(&shim), SecureBootStatus::Trusted { .. }) {
            return Err(Error::Bundle(format!(
                "{SHIM} is not signed by Microsoft's UEFI CA, so stock firmware will refuse to start it"
            )));
        }
        let mm = parse(MOK_MANAGER, &self.mok_manager)?;
        if !mm.signed {
            return Err(Error::Bundle(format!("{MOK_MANAGER} is unsigned; shim will not start it")));
        }
        let loader = parse(LOADER, &self.loader)?;
        if !loader.signed {
            return Err(Error::Bundle(format!("{LOADER} is unsigned; shim will refuse it")));
        }
        if !loader.sections.iter().any(|s| s == ".sbat") {
            return Err(Error::Bundle(format!("{LOADER} has no .sbat section; current shim versions refuse such loaders")));
        }
        let cert = Certificate::from_der(&self.cert)
            .map_err(|_| Error::Bundle(format!("{CERT} is not a DER-encoded X.509 certificate (convert with `openssl x509 -outform DER`)")))?;
        let subject = cert.tbs_certificate.subject.to_string();
        if !loader.signer_subjects.iter().any(|s| *s == subject) {
            return Err(Error::Bundle(format!(
                "{LOADER} was not signed with the certificate in {CERT} (cert subject: {subject}); enrolling it would not help"
            )));
        }
        Ok(())
    }
}
