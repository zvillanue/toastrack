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
    /// Optional Windows helper (`toastrack-attach.exe`). Without it Windows ISOs cannot run Setup from the stick.
    pub attach_exe: Option<Vec<u8>>,
    /// False for a development stick without Secure Boot: the loader alone is the boot file (no shim, no key).
    pub secure_boot: bool,
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
            attach_exe: fs::read(dir.join(crate::windows::BUNDLE_HELPER)).ok(),
            secure_boot: true,
        };
        b.validate()?;
        Ok(b)
    }

    /// Where a release package keeps its bundle: a `bundle` folder next to the program, in its parent's `share/toastrack`, or in
    /// the current directory. Returns the first that holds a loader.
    pub fn discover() -> Option<std::path::PathBuf> {
        let mut roots = Vec::new();
        if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            roots.push(dir.join("bundle"));
            if let Some(up) = dir.parent() {
                roots.push(up.join("share").join("toastrack").join("bundle"));
            }
        }
        roots.push(std::path::PathBuf::from("bundle"));
        roots.into_iter().find(|d| d.join(LOADER).is_file())
    }

    /// A stick for machines with Secure Boot off: `loader` (unsigned is fine) becomes `EFI/BOOT/BOOTX64.EFI`.
    pub fn unsigned(loader_path: &Path, attach_exe: Option<&Path>) -> Result<Self> {
        let read_file = |p: &Path| fs::read(p).map_err(|e| Error::Bundle(format!("{}: {e}", p.display())));
        let b = Self {
            shim: Vec::new(),
            mok_manager: Vec::new(),
            loader: read_file(loader_path)?,
            cert: Vec::new(),
            attach_exe: match attach_exe {
                Some(p) => Some(read_file(p)?),
                None => None,
            },
            secure_boot: false,
        };
        b.validate()?;
        Ok(b)
    }

    /// Checks that the pieces fit together for Secure Boot.
    pub fn validate(&self) -> Result<()> {
        if !self.secure_boot {
            parse(LOADER, &self.loader)?;
            if let Some(exe) = &self.attach_exe {
                parse(crate::windows::BUNDLE_HELPER, exe)?;
            }
            return Ok(());
        }
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
        if let Some(exe) = &self.attach_exe {
            let info = parse(crate::windows::BUNDLE_HELPER, exe)?;
            if info.signed {
                // harmless, but a signed helper would mean the wrong file was bundled
                return Err(Error::Bundle(format!("{} is signed; expected the unsigned helper built by crates/winattach", crate::windows::BUNDLE_HELPER)));
            }
        }
        Ok(())
    }
}
