//! Boot an ISO by presenting it as a virtual disk and starting its own EFI loader.

use crate::{vdisk, vol};
use toastrack_core::{bootcfg::{self, BootEntry}, iso::Iso};
use alloc::{format, string::String, vec::Vec};
use uefi::{
    boot::{self, LoadImageSource, SearchType},
    cstr16,
    proto::{
        device_path::{build::{media::FilePath, DevicePathBuilder}, DevicePath},
        media::{file::{File, FileAttribute, FileMode}, fs::SimpleFileSystem},
    },
    Identify,
};

const LOADER_PATHS: [&uefi::CStr16; 2] = [cstr16!("\\EFI\\BOOT\\BOOTX64.EFI"), cstr16!("\\efi\\boot\\bootx64.efi")];

/// Chainload the ISO's own EFI loader. With `entry`, the ISO's `grub.cfg` is replaced (in the virtual
/// disk only) by a config that boots just that entry with the ISO's path filled in, so the distro's own
/// signed GRUB starts the kernel - the only route that works under Secure Boot.
pub fn boot_iso(vol_h: &vol::DataVolume, dir: &[String], name: &str, entry: Option<&BootEntry>) -> Result<(), String> {
    let mut patches = Vec::new();
    if let Some(entry) = entry {
        let mut iso = Iso::open(vol_h.open_file(dir, name).map_err(|e| format!("open: {e}"))?).map_err(|e| format!("{e}"))?;
        let cfg = iso.find("boot/grub/grub.cfg").map_err(|e| format!("{e}"))?.ok_or("ISO has no boot/grub/grub.cfg to rewrite")?;
        let loc = vol_h.iso_location(dir, name)?;
        let mut text = bootcfg::single_entry_config(entry, &loc)?.into_bytes();
        if text.len() > cfg.size as usize {
            return Err(format!("generated config ({} bytes) does not fit the ISO's grub.cfg ({} bytes)", text.len(), cfg.size));
        }
        text.resize(cfg.size as usize, b'\n'); // keep the recorded file size
        patches.push(vdisk::Patch { offset: cfg.lba as u64 * 2048, data: text });
    }
    let f = vol_h.open_file(dir, name).map_err(|e| format!("open: {e}"))?;
    let size = f.size();
    let disk = vdisk::install(f, size, patches).map_err(|e| format!("install virtual disk: {e:?}"))?;

    // Let the firmware's partition + ISO9660/UDF drivers build filesystems on it.
    boot::connect_controller(disk.handle, &[], None, true).map_err(|e| format!("connect: {e:?}"))?;

    let handles = boot::locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))
        .map_err(|e| format!("locate fs: {e:?}"))?;
    for &h in handles.iter() {
        let Ok(dp) = vol::open_shared::<DevicePath>(h) else { continue };
        if !dp.as_bytes().starts_with(disk.prefix) {
            continue;
        }
        let Ok(mut root) = vol::open_shared::<SimpleFileSystem>(h).and_then(|mut s| s.open_volume()) else { continue };
        for p in LOADER_PATHS {
            if root.open(p, FileMode::Read, FileAttribute::empty()).is_err() {
                continue;
            }
            let mut buf: Vec<u8> = Vec::new();
            let mut b = DevicePathBuilder::with_vec(&mut buf);
            for node in dp.node_iter() {
                b = b.push(&node).map_err(|_| String::from("path build"))?;
            }
            let full = b
                .push(&FilePath { path_name: p })
                .and_then(|b| b.finalize())
                .map_err(|_| String::from("path build"))?;
            let img = boot::load_image(
                boot::image_handle(),
                LoadImageSource::FromDevicePath { device_path: full, boot_policy: uefi::proto::BootPolicy::ExactMatch },
            )
            .map_err(|e| format!("LoadImage failed: {e:?} (Secure Boot rejection?)"))?;
            return boot::start_image(img).map_err(|e| format!("StartImage: {e:?}"));
        }
    }
    Err("no EFI loader found inside the ISO".into())
}

