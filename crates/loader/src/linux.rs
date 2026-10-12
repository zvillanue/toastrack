//! Boot a Linux kernel straight out of an ISO, telling the initramfs where the ISO file lives.

use crate::vol;
use alloc::{boxed::Box, format, string::String, vec::Vec};
use core::ffi::c_void;
use toastrack_core::{bootcfg::BootEntry, iso::Iso};
use uefi::{
    boot::{self, LoadImageSource},
    proto::loaded_image::LoadedImage,
    Guid, Status,
};
use uefi_raw::{
    guid,
    protocol::{device_path::DevicePathProtocol, media::LoadFile2Protocol},
    Boolean,
};

/// The Linux EFI stub looks for this vendor media path to fetch its initrd.
const LINUX_EFI_INITRD_MEDIA_GUID: Guid = guid!("5568e427-68fc-4f3d-ac74-ca555231cc68");

#[repr(C)]
struct InitrdProvider {
    proto: LoadFile2Protocol, // first: `this` is cast back
    data: &'static [u8],
}

unsafe extern "efiapi" fn load_initrd(
    this: *mut LoadFile2Protocol,
    _path: *const DevicePathProtocol,
    boot_policy: Boolean,
    size: *mut usize,
    buf: *mut c_void,
) -> Status {
    if bool::from(boot_policy) || size.is_null() {
        return Status::UNSUPPORTED;
    }
    // SAFETY: `this` is the first field of a leaked InitrdProvider.
    let p = unsafe { &*(this as *const InitrdProvider) };
    // SAFETY: `size` is a valid pointer per the protocol.
    unsafe {
        if buf.is_null() || *size < p.data.len() {
            *size = p.data.len();
            return Status::BUFFER_TOO_SMALL;
        }
        core::ptr::copy_nonoverlapping(p.data.as_ptr(), buf as *mut u8, p.data.len());
        *size = p.data.len();
    }
    Status::SUCCESS
}

fn read_iso_file(iso: &mut Iso<vol::IsoFile>, path: &str) -> Result<Vec<u8>, String> {
    let e = iso
        .find(path.trim_start_matches('/'))
        .map_err(|e| format!("{path}: {e}"))?
        .ok_or_else(|| format!("{path}: not found in ISO"))?;
    iso.read_file(&e).map_err(|e| format!("{path}: {e}"))
}

fn install_initrd(data: Vec<u8>) -> Result<(), String> {
    // Vendor-defined media device path: type 4, subtype 3, length 20, GUID, then the end node.
    let mut dp = Vec::with_capacity(24);
    dp.extend_from_slice(&[4, 3, 20, 0]);
    dp.extend_from_slice(&LINUX_EFI_INITRD_MEDIA_GUID.to_bytes());
    dp.extend_from_slice(&[0x7f, 0xff, 4, 0]);
    let dp: &'static [u8] = Box::leak(dp.into_boxed_slice());
    let prov = Box::leak(Box::new(InitrdProvider {
        proto: LoadFile2Protocol { load_file: load_initrd },
        data: Box::leak(data.into_boxed_slice()),
    }));
    // SAFETY: all interfaces are leaked and outlive boot services.
    unsafe {
        let h = boot::install_protocol_interface(None, &DevicePathProtocol::GUID, dp.as_ptr().cast())
            .map_err(|e| format!("install initrd path: {e:?}"))?;
        boot::install_protocol_interface(Some(h), &LoadFile2Protocol::GUID, (prov as *const InitrdProvider).cast())
            .map_err(|e| format!("install initrd protocol: {e:?}"))?;
    }
    Ok(())
}

pub fn boot_entry(vol_h: &vol::DataVolume, dir: &[String], name: &str, entry: &BootEntry, cmdline: &str) -> Result<(), String> {
    let f = vol_h.open_file(dir, name).map_err(|e| format!("open ISO: {e}"))?;
    let mut iso = Iso::open(f).map_err(|e| format!("{e}"))?;

    let kernel = read_iso_file(&mut iso, &entry.kernel)?;
    let mut initrd = Vec::new();
    for p in &entry.initrds {
        initrd.extend_from_slice(&read_iso_file(&mut iso, p)?);
        initrd.resize(initrd.len().next_multiple_of(4), 0); // keep cpio archives 4-byte aligned
    }


    if !initrd.is_empty() {
        install_initrd(initrd)?;
    }
    let img = boot::load_image(boot::image_handle(), LoadImageSource::FromBuffer { buffer: &kernel, file_path: None })
        .map_err(|e| format!("LoadImage(kernel): {:?} (the firmware does not trust this kernel's signature)", e.status()))?;
    drop(kernel);

    let units: Vec<u16> = cmdline.encode_utf16().chain(core::iter::once(0)).collect();
    let units: &'static [u16] = Box::leak(units.into_boxed_slice());
    let mut li = boot::open_protocol_exclusive::<LoadedImage>(img).map_err(|e| format!("loaded image: {e:?}"))?;
    // SAFETY: the options are leaked and live until the kernel takes over.
    unsafe { li.set_load_options(units.as_ptr().cast(), (units.len() * 2) as u32) };
    drop(li);
    boot::start_image(img).map_err(|e| format!("StartImage(kernel): {e:?}"))
}
