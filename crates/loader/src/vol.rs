//! The data volume: directory listing and file access, over either a firmware-provided FAT filesystem
//! (SimpleFileSystem) or our own exFAT reader running on raw block access (no firmware exFAT driver needed).

use alloc::{string::{String, ToString}, vec::Vec};
use core::cell::RefCell;
use toastrack_core::{
    profile::{Fs, Placement},
    exfat::{DirEntry, ExfatFile, ExfatVolume},
    IoError, ReadAt,
};
use uefi::{
    boot::{self, OpenProtocolAttributes, OpenProtocolParams, ScopedProtocol, SearchType},
    proto::{
        media::{
            block::BlockIO,
            file::{Directory, File, FileAttribute, FileMode, FileSystemVolumeLabel, RegularFile},
            fs::SimpleFileSystem,
        },
    },
    CString16, Handle, Identify,
};

pub const DATA_VOLUME_LABEL: &str = "TOASTRACK";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Up,
    Dir,
    Iso,
    /// A boot entry inside the ISO currently opened in the menu.
    Entry,
    /// Start the ISO's own EFI bootloader instead.
    Chain,
}

#[derive(Clone)]
pub struct Item {
    pub name: String,
    pub kind: Kind,
    pub size: u64,
}

/// Open a protocol without disturbing the drivers that already use it.
pub fn open_shared<P: uefi::proto::ProtocolPointer + ?Sized>(h: Handle) -> uefi::Result<ScopedProtocol<P>> {
    // SAFETY: GetProtocol never disconnects drivers or takes exclusive ownership.
    unsafe {
        boot::open_protocol::<P>(
            OpenProtocolParams { handle: h, agent: boot::image_handle(), controller: None },
            OpenProtocolAttributes::GetProtocol,
        )
    }
}

// ---------------------------------------------------------------- raw block access

/// Byte-addressed reads on top of a BlockIO handle (a partition or a whole disk).
#[derive(Clone)]
pub struct BlockSource {
    handle: Handle,
}

const BOUNCE: usize = 256 * 1024;

impl ReadAt for BlockSource {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), IoError> {
        let mut bio = open_shared::<BlockIO>(self.handle).map_err(|_| IoError)?;
        let m = bio.media();
        let (bs, id, io_align) = (m.block_size() as u64, m.media_id(), (m.io_align() as usize).max(1));
        if bs == 0 || !m.is_media_present() {
            return Err(IoError);
        }
        let end = offset.checked_add(buf.len() as u64).ok_or(IoError)?;
        if end.div_ceil(bs) > m.last_block() + 1 {
            return Err(IoError);
        }
        let mut bounce: Option<alloc::vec::Vec<u64>> = None;
        let (mut pos, mut done) = (offset, 0usize);
        while done < buf.len() {
            let (lba, head) = (pos / bs, (pos % bs) as usize);
            let remaining = buf.len() - done;
            let direct = head == 0
                && remaining as u64 >= bs
                && (buf[done..].as_ptr() as usize) % io_align == 0;
            if direct {
                let n = (remaining / bs as usize * bs as usize).min(8 << 20);
                bio.read_blocks(id, lba, &mut buf[done..done + n]).map_err(|_| IoError)?;
                done += n;
                pos += n as u64;
            } else {
                let b = bounce.get_or_insert_with(|| alloc::vec![0u64; BOUNCE / 8]);
                // SAFETY: a u64 buffer viewed as bytes.
                let bytes = unsafe { core::slice::from_raw_parts_mut(b.as_mut_ptr() as *mut u8, BOUNCE) };
                let blocks = ((head + remaining).div_ceil(bs as usize)).min(BOUNCE / bs as usize);
                let n = blocks * bs as usize;
                bio.read_blocks(id, lba, &mut bytes[..n]).map_err(|_| IoError)?;
                let take = remaining.min(n - head);
                buf[done..done + take].copy_from_slice(&bytes[head..head + take]);
                done += take;
                pos += take as u64;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- volumes

pub enum DataVolume {
    /// Firmware FAT filesystem.
    Sfs(Handle),
    /// exFAT, read by us.
    Exfat(RefCell<ExfatVolume<BlockSource>>),
}

/// Files opened from either kind of volume.
pub enum IsoFile {
    Sfs(RegularFile, u64),
    Exfat(ExfatFile<BlockSource>),
}

impl IsoFile {
    pub fn size(&self) -> u64 {
        match self {
            IsoFile::Sfs(_, s) => *s,
            IsoFile::Exfat(f) => f.size(),
        }
    }
}

impl ReadAt for IsoFile {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), IoError> {
        match self {
            IsoFile::Exfat(f) => f.read_at(offset, buf),
            IsoFile::Sfs(file, _) => {
                file.set_position(offset).map_err(|_| IoError)?;
                let mut done = 0;
                while done < buf.len() {
                    let n = file.read(&mut buf[done..]).map_err(|_| IoError)?;
                    if n == 0 {
                        return Err(IoError);
                    }
                    done += n;
                }
                Ok(())
            }
        }
    }
}

fn open_root(h: Handle) -> uefi::Result<Directory> {
    open_shared::<SimpleFileSystem>(h)?.open_volume()
}

fn kind_of(name: &str, is_dir: bool) -> Option<Kind> {
    if is_dir {
        Some(Kind::Dir)
    } else if name.len() > 4 && name[name.len() - 4..].eq_ignore_ascii_case(".iso") {
        Some(Kind::Iso)
    } else {
        None
    }
}

fn sorted(mut found: Vec<Item>, with_up: bool) -> Vec<Item> {
    found.sort_by(|a, b| (a.kind == Kind::Iso, a.name.to_ascii_lowercase()).cmp(&(b.kind == Kind::Iso, b.name.to_ascii_lowercase())));
    let mut items = Vec::new();
    if with_up {
        items.push(Item { name: "..".into(), kind: Kind::Up, size: 0 });
    }
    items.extend(found);
    items
}

fn sfs_open_dir(h: Handle, path: &[String]) -> uefi::Result<Directory> {
    let mut dir = open_root(h)?;
    for part in path {
        let name = CString16::try_from(part.as_str()).map_err(|_| uefi::Status::INVALID_PARAMETER)?;
        dir = dir.open(&name, FileMode::Read, FileAttribute::empty())?.into_directory().ok_or(uefi::Status::NOT_FOUND)?;
    }
    Ok(dir)
}

impl DataVolume {
    pub fn kind(&self) -> &'static str {
        match self {
            DataVolume::Sfs(_) => "FAT",
            DataVolume::Exfat(_) => "exFAT",
        }
    }

    pub fn list(&self, path: &[String]) -> Result<Vec<Item>, String> {
        match self {
            DataVolume::Sfs(h) => {
                let mut dir = sfs_open_dir(*h, path).map_err(|e| alloc::format!("{e:?}"))?;
                let mut found = Vec::new();
                while let Some(info) = dir.read_entry_boxed().map_err(|e| alloc::format!("{e:?}"))? {
                    let name = info.file_name().to_string();
                    if name == "." || name == ".." {
                        continue;
                    }
                    if let Some(kind) = kind_of(&name, info.is_directory()) {
                        found.push(Item { name, kind, size: info.file_size() });
                    }
                }
                Ok(sorted(found, !path.is_empty()))
            }
            DataVolume::Exfat(v) => {
                let mut v = v.borrow_mut();
                let dir = Self::exfat_dir(&mut v, path)?;
                let found = v
                    .read_dir(&dir)
                    .map_err(|e| alloc::format!("{e}"))?
                    .into_iter()
                    .filter_map(|e| kind_of(&e.name, e.is_dir).map(|kind| Item { name: e.name, kind, size: e.size }))
                    .collect();
                Ok(sorted(found, !path.is_empty()))
            }
        }
    }

    fn exfat_dir(v: &mut ExfatVolume<BlockSource>, path: &[String]) -> Result<toastrack_core::exfat::Dir, String> {
        let mut dir = v.root();
        for part in path {
            let e = v
                .read_dir(&dir)
                .map_err(|e| alloc::format!("{e}"))?
                .into_iter()
                .find(|e| e.is_dir && e.name == *part)
                .ok_or_else(|| alloc::format!("folder {part} not found"))?;
            dir = v.dir_of(&e).ok_or("not a folder")?;
        }
        Ok(dir)
    }

    /// Where an ISO file sits on the stick, in the terms a booted Linux needs (see `toastrack_core::profile`).
    pub fn placement(&self, path: &[String], name: &str) -> Result<Placement, String> {
        let mut p = String::from("/");
        for d in path {
            p.push_str(d);
            p.push('/');
        }
        p.push_str(name);
        match self {
            DataVolume::Sfs(h) => Ok(Placement { path: p, fs_uuid: fat_uuid(*h).unwrap_or_default(), fs: Fs::Fat, raw_offset: None, fragments: 1 }),
            DataVolume::Exfat(v) => {
                let IsoFile::Exfat(mut f) = self.open_file(path, name)? else { return Err("not an exFAT file".into()) };
                let runs = f.extents().map_err(|e| alloc::format!("{e}"))?;
                let serial = v.borrow().serial();
                Ok(Placement {
                    path: p,
                    fs_uuid: alloc::format!("{:04X}-{:04X}", serial >> 16, serial & 0xFFFF),
                    fs: Fs::Exfat,
                    raw_offset: if let [(off, _)] = runs.as_slice() { Some(*off) } else { None },
                    fragments: runs.len(),
                })
            }
        }
    }

    pub fn open_file(&self, path: &[String], name: &str) -> Result<IsoFile, String> {
        match self {
            DataVolume::Sfs(h) => {
                let mut dir = sfs_open_dir(*h, path).map_err(|e| alloc::format!("{e:?}"))?;
                let n = CString16::try_from(name).map_err(|_| "bad file name")?;
                let mut file = dir
                    .open(&n, FileMode::Read, FileAttribute::empty())
                    .map_err(|e| alloc::format!("{e:?}"))?
                    .into_regular_file()
                    .ok_or("not a file")?;
                let size = file.get_boxed_info::<uefi::proto::media::file::FileInfo>().map_err(|e| alloc::format!("{e:?}"))?.file_size();
                Ok(IsoFile::Sfs(file, size))
            }
            DataVolume::Exfat(v) => {
                let mut v = v.borrow_mut();
                let dir = Self::exfat_dir(&mut v, path)?;
                let e: DirEntry = v
                    .read_dir(&dir)
                    .map_err(|e| alloc::format!("{e}"))?
                    .into_iter()
                    .find(|e| !e.is_dir && e.name == name)
                    .ok_or_else(|| alloc::format!("{name} not found"))?;
                Ok(IsoFile::Exfat(v.open_file(&e)))
            }
        }
    }
}

/// The volume UUID blkid would print for a FAT volume (`ABCD-1234`), read from its boot sector.
fn fat_uuid(handle: Handle) -> Option<String> {
    let mut b = [0u8; 512];
    BlockSource { handle }.read_at(0, &mut b).ok()?;
    if b[510] != 0x55 || b[511] != 0xAA {
        return None;
    }
    let off = if &b[0x52..0x57] == b"FAT32" { 0x43 } else if &b[0x36..0x39] == b"FAT" { 0x27 } else { return None };
    let id = u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]]);
    Some(alloc::format!("{:04X}-{:04X}", id >> 16, id & 0xFFFF))
}

// ---------------------------------------------------------------- discovery

fn exfat_on(handle: Handle) -> Option<ExfatVolume<BlockSource>> {
    ExfatVolume::open(BlockSource { handle }).ok()
}

fn is_data_label(l: &str) -> bool {
    l.eq_ignore_ascii_case(DATA_VOLUME_LABEL)
}

/// The volume labelled TOASTRACK (exFAT via our reader, or FAT via the firmware), falling back to the
/// volume the loader started from.
pub fn find_data_volume() -> DataVolume {
    if let Ok(hs) = boot::locate_handle_buffer(SearchType::ByProtocol(&BlockIO::GUID)) {
        // Partitions first; a whole-disk handle only matters for unpartitioned media.
        let mut handles: Vec<(bool, Handle)> = Vec::new();
        for &h in hs.iter() {
            if let Ok(b) = open_shared::<BlockIO>(h) {
                let m = b.media();
                if m.is_media_present() && m.block_size() <= 4096 {
                    handles.push((m.is_logical_partition(), h));
                }
            }
        }
        handles.sort_by_key(|(logical, _)| !*logical);
        for (_, h) in handles {
            if let Some(mut v) = exfat_on(h) {
                if v.label().ok().flatten().is_some_and(|l| is_data_label(&l)) {
                    return DataVolume::Exfat(RefCell::new(v));
                }
            }
        }
    }
    if let Ok(hs) = boot::locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID)) {
        for &h in hs.iter() {
            if let Ok(mut root) = open_root(h) {
                if let Ok(info) = root.get_boxed_info::<FileSystemVolumeLabel>() {
                    if is_data_label(&info.volume_label().to_string()) {
                        return DataVolume::Sfs(h);
                    }
                }
            }
        }
    }
    let li = open_shared::<uefi::proto::loaded_image::LoadedImage>(boot::image_handle()).ok();
    DataVolume::Sfs(li.and_then(|li| li.device()).unwrap_or_else(boot::image_handle))
}
