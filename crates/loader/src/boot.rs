//! Boot an ISO by presenting it as a virtual disk and starting its own EFI loader.

use crate::{vdisk, vol};
use alloc::{format, string::String, vec::Vec};
use uefi::{
    boot::{self, LoadImageSource, SearchType},
    cstr16,
    proto::{
        device_path::{build::{media::FilePath, DevicePathBuilder}, DevicePath},
        media::{file::{File, FileAttribute, FileMode}, fs::SimpleFileSystem},
    },
    Handle, Identify,
};

const LOADER_PATHS: [&uefi::CStr16; 2] = [cstr16!("\\EFI\\BOOT\\BOOTX64.EFI"), cstr16!("\\efi\\boot\\bootx64.efi")];

pub fn boot_iso(vol_h: Handle, dir: &[String], name: &str) -> Result<(), String> {
    let mut f = vol::open_file(vol_h, dir, name).map_err(|e| format!("open: {e:?}"))?;
    let size = file_size(&mut f)?;
    let disk = vdisk::install(f, size).map_err(|e| format!("install virtual disk: {e:?}"))?;

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

fn file_size(f: &mut vol::IsoFile) -> Result<u64, String> {
    use uefi::proto::media::file::FileInfo;
    let info = f.file.get_boxed_info::<FileInfo>().map_err(|e| format!("stat: {e:?}"))?;
    Ok(info.file_size())
}
