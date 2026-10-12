//! The whole install/update/verify operations, shared by the command line and the graphical app.

use crate::{bundle::Bundle, device, install, target::Target, Result};
use std::path::Path;

/// Erase `target` (a disk or an image file) and set it up as a Toastrack stick.
pub fn install_to(target: &Path, bundle: &Bundle, fs: install::DataFs, size: Option<u64>, allow_internal: bool) -> Result<install::Parts> {
    device::check_target(target, allow_internal)?;
    let t = Target::open(target, true, size, true)?;
    let (t, parts) = install::install(t, bundle, fs)?;
    t.finish()?;
    Ok(parts)
}

/// Replace shim, loader and key on an existing stick; the data partition is not touched.
pub fn update_at(target: &Path, bundle: &Bundle, allow_internal: bool) -> Result<()> {
    device::check_target(target, allow_internal)?;
    let t = Target::open(target, true, None, false)?;
    install::update(t, bundle)?.finish()
}

/// Read a stick back and list the differences from the bundle (empty means it matches).
pub fn verify_at(target: &Path, bundle: &Bundle) -> Result<Vec<String>> {
    let t = Target::open(target, false, None, false)?;
    Ok(install::verify(t, bundle)?.1)
}

/// What to tell the user after a successful install, one paragraph per entry.
pub fn next_steps(bundle: &Bundle) -> Vec<String> {
    let mut v = vec!["Copy ISO files into the stick's data partition (volume TOASTRACK), for example into its isos folder, then eject the stick properly.".to_string()];
    if bundle.secure_boot {
        v.push(format!(
            "On the first boot of each PC with Secure Boot on, shim opens MokManager: choose \"Enroll key from disk\", pick {}, confirm, reboot.",
            crate::bundle::CERT
        ));
        if let Some(fp) = bundle.cert_fingerprint() {
            v.push(format!("The key's SHA-256 fingerprint is {fp}. MokManager shows it when you view the key; make sure it matches before you enroll."));
        }
    } else {
        v.push("This stick has no shim: it boots only on PCs with Secure Boot turned off.".to_string());
    }
    v.push(if bundle.attach_exe.is_some() {
        "Windows ISOs: the helper (toastrack\\attach.exe) and autounattend.xml are on the data partition.".to_string()
    } else {
        "No Windows helper on this stick: Windows ISOs will start, but Setup cannot find its install files.".to_string()
    });
    v
}
