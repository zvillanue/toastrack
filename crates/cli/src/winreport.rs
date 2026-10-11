//! `toastrack windows-report`: facts about a Windows ISO, in a form that is safe to paste into an issue.
//! Only names, sizes, header fields and the boot manager's signature data are printed - never file contents.

use std::{fmt::Write as _, fs::File, io::BufReader, path::Path};
use toastrack_core::{
    fat::FatImage,
    iso::{Iso, IsoRegion},
    pe,
    udf::{UdfFileReader, UdfVolume},
    verdict::classify,
    wim,
};

const KEY_FILES: [&str; 12] = [
    "bootmgr",
    "bootmgr.efi",
    "boot/bcd",
    "boot/boot.sdi",
    "boot/etfsboot.com",
    "efi/boot/bootx64.efi",
    "efi/microsoft/boot/bcd",
    "efi/microsoft/boot/efisys.bin",
    "sources/boot.wim",
    "sources/install.wim",
    "sources/install.esd",
    "sources/setup.exe",
];

fn gib(b: u64) -> String {
    format!("{:.2} GiB", b as f64 / (1u64 << 30) as f64)
}

pub fn report(path: &Path) -> Result<String, String> {
    let mut o = String::new();
    let open = || File::open(path).map_err(|e| e.to_string());
    let _ = writeln!(o, "Toastrack Windows ISO report ({})", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(o, "file: {}  size: {}", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(), gib(open()?.metadata().map_err(|e| e.to_string())?.len()));

    // --- ISO 9660 side (a stub on real Windows media) and El Torito.
    let mut iso = Iso::open(BufReader::new(open()?)).map_err(|e| format!("not an ISO 9660 image: {e}"))?;
    let _ = writeln!(o, "\n== ISO 9660 / El Torito");
    let root = iso.find("").map_err(|e| e.to_string())?.ok_or("no root")?;
    let names: Vec<String> = iso.read_dir(&root).map_err(|e| e.to_string())?.into_iter().map(|e| e.name).collect();
    let _ = writeln!(o, "ISO 9660 top level ({} entries): {}", names.len(), names.iter().take(12).cloned().collect::<Vec<_>>().join(", "));
    let efi_images = iso.efi_boot_images().map_err(|e| e.to_string())?;
    let _ = writeln!(o, "El Torito UEFI boot images: {}", efi_images.len());

    // --- Boot manager: from the tree, or from the El Torito EFI image.
    let _ = writeln!(o, "\n== EFI boot manager");
    let mut loader: Option<(String, Vec<u8>)> = None;
    if let Ok(Some(e)) = iso.find("EFI/BOOT/BOOTX64.EFI") {
        if let Ok(d) = iso.read_file(&e) {
            loader = Some(("ISO 9660 tree EFI/BOOT/BOOTX64.EFI".into(), d));
        }
    }
    for (n, off) in efi_images.iter().enumerate() {
        match FatImage::open(IsoRegion { iso: &mut iso, off: *off }) {
            Ok(mut fat) => {
                let top: Vec<String> = fat.list_dir("").unwrap_or_default().into_iter().map(|e| e.name).collect();
                let boot: Vec<String> = fat.list_dir("EFI/Microsoft/Boot").unwrap_or_default().into_iter().map(|e| format!("{}{}", e.name, if e.is_dir { "/" } else { "" })).collect();
                let _ = writeln!(o, "EFI image #{n}: FAT at byte {off}; top level: {}", top.join(", "));
                if !boot.is_empty() {
                    let _ = writeln!(o, "  EFI/Microsoft/Boot: {}", boot.join(", "));
                }
                if loader.is_none() {
                    if let Ok(Some(e)) = fat.find("EFI/BOOT/BOOTX64.EFI") {
                        if let Ok(d) = fat.read_file(&e) {
                            loader = Some((format!("El Torito EFI image #{n} EFI/BOOT/BOOTX64.EFI"), d));
                        }
                    }
                }
            }
            Err(e) => {
                let _ = writeln!(o, "EFI image #{n}: not a FAT image ({e:?})");
            }
        }
    }
    match loader {
        None => {
            let _ = writeln!(o, "no x86-64 EFI/BOOT/BOOTX64.EFI found");
        }
        Some((where_, data)) => match pe::inspect(&data) {
            Ok(info) => {
                let status = classify(&info);
                let _ = writeln!(o, "loader: {where_} ({} bytes)", data.len());
                let _ = writeln!(o, "  authenticode sha256: {}", info.authenticode_sha256.iter().map(|b| format!("{b:02x}")).collect::<String>());
                let _ = writeln!(o, "  signers: {}", if info.signer_subjects.is_empty() { "(unsigned)".into() } else { info.signer_subjects.join(" | ") });
                let _ = writeln!(o, "  secure boot: {:?}; windows boot manager: {}", status.boots_with_secure_boot(), status.is_windows());
                let joined = info.signer_subjects.join(" ");
                if joined.contains("Windows Production PCA 2011") {
                    let _ = writeln!(o, "  note: signed through the 2011 Windows PCA; PCs with the boot-manager revocation updates may refuse it");
                }
                if joined.contains("Windows UEFI CA 2023") {
                    let _ = writeln!(o, "  note: signed through the 2023 CA; firmware without the 2023 db update will not trust it");
                }
            }
            Err(e) => {
                let _ = writeln!(o, "loader: {where_} is not a valid PE image ({e:?})");
            }
        },
    }

    // --- UDF side: the real file tree on Windows media.
    let _ = writeln!(o, "\n== UDF file tree");
    match UdfVolume::open(open()?) {
        Err(e) => {
            let _ = writeln!(o, "UDF: {e}");
        }
        Ok(mut udf) => {
            let top: Vec<String> = udf.read_dir(&udf.root()).map_err(|e| e.to_string())?.into_iter().map(|e| format!("{}{}", e.name, if e.is_dir { "/" } else { "" })).collect();
            let _ = writeln!(o, "UDF top level: {}", top.join(", "));
            for p in KEY_FILES {
                match udf.find(p).map_err(|e| e.to_string())? {
                    Some(e) => {
                        let _ = writeln!(o, "  {p:<34} {:>14} bytes{}", e.size, if e.size > 0xFFFF_FFFF { "  (over 4 GiB: does not fit FAT32 unsplit)" } else { "" });
                    }
                    None => {
                        let _ = writeln!(o, "  {p:<34} -");
                    }
                }
            }
            for p in ["sources/boot.wim", "sources/install.wim", "sources/install.esd"] {
                let Some(e) = udf.find(p).map_err(|e| e.to_string())? else { continue };
                let len = e.size;
                match wim::read_info(&mut UdfFileReader { vol: &mut udf, entry: e }, len) {
                    None => {
                        let _ = writeln!(o, "\n== {p}\nnot a WIM header");
                    }
                    Some(w) => {
                        let _ = writeln!(o, "\n== {p}");
                        let _ = writeln!(
                            o,
                            "WIM version {:#x}, compression {}, chunk {} bytes, parts {}/{}, images {}, boot index {}",
                            w.version, w.compression(), w.chunk_size, w.part_number, w.total_parts, w.image_count, w.boot_index
                        );
                        for i in &w.images {
                            let _ = writeln!(o, "  image {}: {:?} build {:?} arch {:?}", i.index, i.name, i.build, i.arch);
                        }
                    }
                }
            }
        }
    }
    let _ = writeln!(o, "\n(end of report; it contains no file contents)");
    Ok(o)
}
