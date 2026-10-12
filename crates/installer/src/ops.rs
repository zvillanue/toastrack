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
