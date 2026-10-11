//! Volume discovery, directory listing and file access on top of SimpleFileSystem.

use alloc::{string::{String, ToString}, vec::Vec};
use toastrack_core::{IoError, ReadAt};
use uefi::{
    CString16,
    boot::{self, OpenProtocolAttributes, OpenProtocolParams, ScopedProtocol, SearchType},
    proto::media::{
        file::{Directory, File, FileAttribute, FileMode, FileSystemVolumeLabel, RegularFile},
        fs::SimpleFileSystem,
    },
    Handle, Identify,
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

fn open_root(h: Handle) -> uefi::Result<Directory> {
    open_shared::<SimpleFileSystem>(h)?.open_volume()
}

/// The volume labelled TOASTRACK, falling back to the volume the loader started from.
pub fn find_data_volume() -> Handle {
    if let Ok(hs) = boot::locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID)) {
        for &h in hs.iter() {
            if let Ok(mut root) = open_root(h) {
                if let Ok(info) = root.get_boxed_info::<FileSystemVolumeLabel>() {
                    if info.volume_label().to_string().eq_ignore_ascii_case(DATA_VOLUME_LABEL) {
                        return h;
                    }
                }
            }
        }
    }
    let li = open_shared::<uefi::proto::loaded_image::LoadedImage>(boot::image_handle()).ok();
    li.and_then(|li| li.device()).unwrap_or_else(boot::image_handle)
}

fn open_dir(h: Handle, path: &[String]) -> uefi::Result<Directory> {
    let mut dir = open_root(h)?;
    for part in path {
        let name = CString16::try_from(part.as_str()).map_err(|_| uefi::Status::INVALID_PARAMETER)?;
        dir = dir
            .open(&name, FileMode::Read, FileAttribute::empty())?
            .into_directory()
            .ok_or(uefi::Status::NOT_FOUND)?;
    }
    Ok(dir)
}

pub fn list(h: Handle, path: &[String]) -> uefi::Result<Vec<Item>> {
    let mut dir = open_dir(h, path)?;
    let mut items = Vec::new();
    if !path.is_empty() {
        items.push(Item { name: "..".into(), kind: Kind::Up, size: 0 });
    }
    let mut found = Vec::new();
    while let Some(info) = dir.read_entry_boxed()? {
        let name = info.file_name().to_string();
        if name == "." || name == ".." {
            continue;
        }
        let kind = if info.is_directory() {
            Kind::Dir
        } else if name.len() > 4 && name[name.len() - 4..].eq_ignore_ascii_case(".iso") {
            Kind::Iso
        } else {
            continue;
        };
        found.push(Item { name, kind, size: info.file_size() });
    }
    found.sort_by(|a, b| {
        (a.kind == Kind::Iso, a.name.to_ascii_lowercase()).cmp(&(b.kind == Kind::Iso, b.name.to_ascii_lowercase()))
    });
    items.extend(found);
    Ok(items)
}

pub struct IsoFile {
    pub file: RegularFile,
}

pub fn open_file(h: Handle, path: &[String], name: &str) -> uefi::Result<IsoFile> {
    let mut dir = open_dir(h, path)?;
    let n = CString16::try_from(name).map_err(|_| uefi::Status::INVALID_PARAMETER)?;
    let file = dir
        .open(&n, FileMode::Read, FileAttribute::empty())?
        .into_regular_file()
        .ok_or(uefi::Status::NOT_FOUND)?;
    Ok(IsoFile { file })
}

impl ReadAt for IsoFile {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), IoError> {
        self.file.set_position(offset).map_err(|_| IoError)?;
        let mut done = 0;
        while done < buf.len() {
            let n = self.file.read(&mut buf[done..]).map_err(|_| IoError)?;
            if n == 0 {
                return Err(IoError);
            }
            done += n;
        }
        Ok(())
    }
}
