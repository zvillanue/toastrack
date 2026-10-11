//! Writing, updating and verifying a stick.

use crate::{
    bundle::{self, Bundle},
    exfat_format::{self, Node},
    layout::{self, Layout, SECTOR},
    Error, Result,
};
use fatfs::{FatType, FileSystem, FormatVolumeOptions, FsOptions};
use fscommon::{BufStream, StreamSlice};
use gpt::{disk::LogicalBlockSize, mbr::ProtectiveMBR, partition_types, GptConfig};
use sha2::{Digest, Sha256};
use toastrack_core::{exfat::ExfatVolume, io::{Shared, Window}};
use std::{
    fmt::Debug,
    io::{Read, Seek, SeekFrom, Write},
};

/// Filesystem of the data partition. exFAT allows files over 4 GB (needed for many ISOs);
/// FAT32 is what every firmware can read without help.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DataFs {
    #[default]
    Exfat,
    Fat32,
}

const README: &[u8] = b"Copy ISO files here. Sub-folders are shown as folders in the Toastrack menu.\r\n";

pub trait Device: Read + Write + Seek + Debug {}
impl<T: Read + Write + Seek + Debug> Device for T {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parts {
    pub esp: (u64, u64),  // byte offset, length
    pub data: (u64, u64),
}

/// Volume serial numbers must differ between sticks, or /dev/disk/by-uuid links collide.
fn volume_id(salt: u32) -> u32 {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut h = Sha256::new();
    h.update(t.to_le_bytes());
    h.update(std::process::id().to_le_bytes());
    h.update(salt.to_le_bytes());
    let d = h.finalize();
    u32::from_le_bytes([d[0], d[1], d[2], d[3]])
}

fn gpt_err(e: impl ToString) -> Error {
    Error::Gpt(e.to_string())
}
fn fat_err(e: impl ToString) -> Error {
    Error::Fat(e.to_string())
}

pub fn device_size<D: Seek>(d: &mut D) -> Result<u64> {
    let n = d.seek(SeekFrom::End(0))?;
    d.seek(SeekFrom::Start(0))?;
    Ok(n)
}

fn slice<'a, D: Device>(d: &'a mut D, (off, len): (u64, u64)) -> Result<BufStream<StreamSlice<&'a mut D>>> {
    Ok(BufStream::new(StreamSlice::new(d, off, off + len)?))
}

fn write_file<T: fatfs::ReadWriteSeek>(fs: &FileSystem<T>, path: &str, data: &[u8]) -> Result<()> {
    let root = fs.root_dir();
    let (dir, name) = match path.rsplit_once('/') {
        Some((d, n)) => {
            let mut cur = root;
            for part in d.split('/') {
                cur = cur.create_dir(part).map_err(fat_err)?; // create_dir opens it if it already exists
            }
            (cur, n)
        }
        None => (root, path),
    };
    if dir.open_file(name).is_ok() {
        dir.remove(name).map_err(fat_err)?;
    }
    let mut f = dir.create_file(name).map_err(fat_err)?;
    f.write_all(data)?;
    f.flush()?;
    Ok(())
}

fn write_esp<D: Device>(dev: &mut D, esp: (u64, u64), b: &Bundle) -> Result<()> {
    let fs = FileSystem::new(slice(dev, esp)?, FsOptions::new()).map_err(fat_err)?;
    write_file(&fs, &format!("EFI/BOOT/{}", "BOOTX64.EFI"), &b.shim)?; // firmware default boot file = shim
    write_file(&fs, &format!("EFI/BOOT/{}", bundle::MOK_MANAGER), &b.mok_manager)?;
    write_file(&fs, &format!("EFI/BOOT/{}", bundle::LOADER), &b.loader)?;
    write_file(&fs, bundle::CERT, &b.cert)?;
    fs.unmount().map_err(fat_err)
}

/// Create a fresh GPT with an EFI partition and a data partition, format both, and install `bundle`.
/// Everything previously on the device becomes unreachable.
pub fn install<D: Device>(dev: D, bundle: &Bundle, data_fs: DataFs) -> Result<(D, Parts)> {
    bundle.validate()?;
    install_unchecked(dev, bundle, data_fs)
}

/// Like [`install`] without checking the bundle's signatures (for tests with synthetic files).
#[doc(hidden)]
pub fn install_unchecked<D: Device>(mut dev: D, bundle: &Bundle, data_fs: DataFs) -> Result<(D, Parts)> {
    let size = device_size(&mut dev)?;
    let lay = Layout::for_disk(size)?;

    // Wipe the first MiB (old partition tables, boot code, filesystem signatures).
    dev.seek(SeekFrom::Start(0))?;
    dev.write_all(&vec![0u8; layout::MIB as usize])?;

    let lba_count = size / SECTOR - 1;
    ProtectiveMBR::with_lb_size(u32::try_from(lba_count).unwrap_or(u32::MAX))
        .overwrite_lba0(&mut dev)
        .map_err(gpt_err)?;
    let mut gdisk = GptConfig::new()
        .writable(true)
        .logical_block_size(LogicalBlockSize::Lb512)
        .create_from_device(dev, None)
        .map_err(gpt_err)?;
    let align = Some(layout::ALIGN / SECTOR);
    gdisk.add_partition(layout::ESP_NAME, lay.esp_bytes, partition_types::EFI, 0, align).map_err(gpt_err)?;
    gdisk.add_partition(layout::DATA_NAME, lay.data_bytes, partition_types::BASIC, 0, align).map_err(gpt_err)?;
    let parts = parts_of(&gdisk)?;
    let mut dev = gdisk.write().map_err(gpt_err)?;

    // EFI partition: always FAT32 (the firmware must read it).
    {
        let mut s = slice(&mut dev, parts.esp)?;
        let opts = FormatVolumeOptions::new().fat_type(FatType::Fat32).volume_label(*layout::ESP_LABEL).volume_id(volume_id(0));
        fatfs::format_volume(&mut s, opts).map_err(fat_err)?;
        s.flush()?;
    }
    write_esp(&mut dev, parts.esp, bundle)?;

    // Data partition, with an isos/ folder and a first-use hint.
    match data_fs {
        DataFs::Exfat => {
            let mut s = slice(&mut dev, parts.data)?;
            let tree = [Node::Dir("isos", vec![Node::File("README.txt", README)])];
            exfat_format::format(&mut s, parts.data.1, parts.data.0, "TOASTRACK", volume_id(1), &tree)?;
            s.flush()?;
        }
        DataFs::Fat32 => {
            let mut s = slice(&mut dev, parts.data)?;
            let opts = FormatVolumeOptions::new().fat_type(FatType::Fat32).volume_label(*layout::DATA_LABEL).volume_id(volume_id(1));
            fatfs::format_volume(&mut s, opts).map_err(fat_err)?;
            s.flush()?;
            drop(s);
            let fs = FileSystem::new(slice(&mut dev, parts.data)?, FsOptions::new()).map_err(fat_err)?;
            write_file(&fs, "isos/README.txt", README)?;
            fs.unmount().map_err(fat_err)?;
        }
    }
    dev.flush()?;
    Ok((dev, parts))
}

fn parts_of<D: Device>(g: &gpt::GptDisk<D>) -> Result<Parts> {
    let lb = *g.logical_block_size();
    let find = |is_esp: bool| -> Result<(u64, u64)> {
        let p = g
            .partitions()
            .values()
            .find(|p| if is_esp { p.part_type_guid == partition_types::EFI } else { p.name == layout::DATA_NAME })
            .ok_or_else(|| Error::Gpt(format!("no {} partition found; is this a Toastrack stick?", if is_esp { "EFI" } else { "data" })))?;
        Ok((p.bytes_start(lb).map_err(gpt_err)?, p.bytes_len(lb).map_err(gpt_err)?))
    };
    Ok(Parts { esp: find(true)?, data: find(false)? })
}

pub fn read_parts<D: Device>(dev: D) -> Result<(D, Parts)> {
    let g = GptConfig::new()
        .logical_block_size(LogicalBlockSize::Lb512)
        .open_from_device(dev)
        .map_err(gpt_err)?;
    let parts = parts_of(&g)?;
    Ok((g.take_device(), parts))
}

/// Replace shim, loader, MokManager and certificate on an existing stick, leaving the ISOs alone.
pub fn update<D: Device>(dev: D, bundle: &Bundle) -> Result<D> {
    bundle.validate()?;
    update_unchecked(dev, bundle)
}

/// Like [`update`] without checking the bundle's signatures (for tests with synthetic files).
#[doc(hidden)]
pub fn update_unchecked<D: Device>(dev: D, bundle: &Bundle) -> Result<D> {
    let (mut dev, parts) = read_parts(dev)?;
    write_esp(&mut dev, parts.esp, bundle)?;
    dev.flush()?;
    Ok(dev)
}

fn sha(d: &[u8]) -> [u8; 32] {
    Sha256::digest(d).into()
}

/// Read the stick back and compare it with `bundle`. Returns the list of problems (empty = good).
pub fn verify<D: Device>(dev: D, bundle: &Bundle) -> Result<(D, Vec<String>)> {
    let (mut dev, parts) = read_parts(dev)?;
    let mut problems = Vec::new();
    {
        let fs = FileSystem::new(slice(&mut dev, parts.esp)?, FsOptions::new()).map_err(fat_err)?;
        let root = fs.root_dir();
        for (path, want) in [
            ("EFI/BOOT/BOOTX64.EFI", &bundle.shim),
            ("EFI/BOOT/mmx64.efi", &bundle.mok_manager),
            ("EFI/BOOT/grubx64.efi", &bundle.loader),
            (bundle::CERT, &bundle.cert),
        ] {
            let mut got = Vec::new();
            match root.open_file(path) {
                Ok(mut f) => {
                    f.read_to_end(&mut got)?;
                    if sha(&got) != sha(want) {
                        problems.push(format!("{path} differs from the bundle"));
                    }
                }
                Err(_) => problems.push(format!("{path} is missing")),
            }
        }
    }
    dev.seek(SeekFrom::Start(parts.data.0))?;
    let mut boot = [0u8; 512];
    dev.read_exact(&mut boot)?;
    if &boot[3..11] == b"EXFAT   " {
        let win = Window { inner: Shared::new(&mut dev), offset: parts.data.0, len: parts.data.1 };
        match ExfatVolume::open(win).and_then(|mut v| v.label()) {
            Ok(Some(l)) if l == "TOASTRACK" => {}
            other => problems.push(format!("data volume label is {other:?}, the loader looks for \"TOASTRACK\"")),
        }
    } else {
        let fs = FileSystem::new(slice(&mut dev, parts.data)?, FsOptions::new()).map_err(fat_err)?;
        if fs.volume_label().trim() != "TOASTRACK" {
            problems.push(format!("data volume label is {:?}, the loader looks for \"TOASTRACK\"", fs.volume_label()));
        }
        if fs.fat_type() != FatType::Fat32 {
            problems.push(format!("data volume is {:?}, expected FAT32", fs.fat_type()));
        }
    }
    Ok((dev, problems))
}
