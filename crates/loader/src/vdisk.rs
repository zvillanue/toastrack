//! Expose an ISO file as a read-only 2048-byte-sector optical block device.
//!
//! Once installed and connected, the firmware's own partition/ISO9660/UDF drivers
//! create filesystems on it, so the ISO's native EFI loader can be started from it.

use crate::vol::IsoFile;
use alloc::boxed::Box;
use core::ffi::c_void;
use uefi::{
    boot,
    proto::device_path::{build::{hardware::Vendor, DevicePathBuilder}, DevicePath},
    Guid, Handle, Status,
};
use uefi_raw::{
    guid,
    protocol::{
        block::{BlockIoMedia, BlockIoProtocol},
        device_path::DevicePathProtocol,
    },
    Boolean,
};

const SECTOR: u64 = 2048;
const VENDOR: Guid = guid!("5f0b6a2e-3c1d-4b8e-9a77-7a6f1d0c2e41");

#[repr(C)]
struct VDisk {
    bio: BlockIoProtocol, // must stay first: `this` is cast back to VDisk
    media: BlockIoMedia,
    file: IsoFile,
    size: u64,
}

unsafe extern "efiapi" fn reset(_: *mut BlockIoProtocol, _: Boolean) -> Status {
    Status::SUCCESS
}

unsafe extern "efiapi" fn flush(_: *mut BlockIoProtocol) -> Status {
    Status::SUCCESS
}

unsafe extern "efiapi" fn write(_: *mut BlockIoProtocol, _: u32, _: u64, _: usize, _: *const c_void) -> Status {
    Status::WRITE_PROTECTED
}

unsafe extern "efiapi" fn read(this: *const BlockIoProtocol, _: u32, lba: u64, size: usize, buf: *mut c_void) -> Status {
    // SAFETY: `this` points at the first field of a leaked, never-freed VDisk.
    let d = unsafe { &mut *(this as *mut VDisk) };
    if size as u64 % SECTOR != 0 {
        return Status::BAD_BUFFER_SIZE;
    }
    if buf.is_null() || lba * SECTOR + size as u64 > d.size {
        return Status::INVALID_PARAMETER;
    }
    // SAFETY: the caller guarantees `buf` is `size` bytes.
    let out = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, size) };
    use toastrack_core::ReadAt;
    match d.file.read_at(lba * SECTOR, out) {
        Ok(()) => Status::SUCCESS,
        Err(_) => Status::DEVICE_ERROR,
    }
}

pub struct InstalledDisk {
    pub handle: Handle,
    /// Device path bytes without the end node: children's paths start with this.
    pub prefix: &'static [u8],
}

pub fn install(file: IsoFile, size: u64) -> uefi::Result<InstalledDisk> {
    let mut buf = alloc::vec::Vec::new();
    let dp: &DevicePath = DevicePathBuilder::with_vec(&mut buf)
        .push(&Vendor { vendor_guid: VENDOR, vendor_defined_data: b"toastrack-iso" })
        .map_err(|_| Status::OUT_OF_RESOURCES)?
        .finalize()
        .map_err(|_| Status::OUT_OF_RESOURCES)?;
    let bytes: &'static [u8] = Box::leak(dp.as_bytes().to_vec().into_boxed_slice());
    let prefix = &bytes[..bytes.len() - 4]; // strip the 4-byte end node

    let disk = Box::leak(Box::new(VDisk {
        bio: BlockIoProtocol {
            revision: BlockIoProtocol::REVISION,
            media: core::ptr::null(),
            reset,
            read_blocks: read,
            write_blocks: write,
            flush_blocks: flush,
        },
        media: BlockIoMedia {
            media_id: 1,
            removable_media: true.into(),
            media_present: true.into(),
            logical_partition: false.into(),
            read_only: true.into(),
            write_caching: false.into(),
            block_size: SECTOR as u32,
            io_align: 0,
            last_block: size / SECTOR - 1,
            lowest_aligned_lba: 0,
            logical_blocks_per_physical_block: 1,
            optimal_transfer_length_granularity: 0,
        },
        file,
        size: size / SECTOR * SECTOR,
    }));
    disk.bio.media = &disk.media;

    // SAFETY: both interfaces are leaked and live for the rest of the boot.
    unsafe {
        let h = boot::install_protocol_interface(None, &DevicePathProtocol::GUID, bytes.as_ptr().cast())?;
        boot::install_protocol_interface(Some(h), &BlockIoProtocol::GUID, (&disk.bio as *const BlockIoProtocol).cast())?;
        Ok(InstalledDisk { handle: h, prefix })
    }
}
