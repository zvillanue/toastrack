mod winreport;

use clap::Parser;
use std::{
    fs::File,
    io::{BufReader, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};
use toastrack_core::{analyze_iso, iso::Iso, SecureBootStatus};
use toastrack_installer::{bundle::Bundle, device, install, ops, Error};

#[derive(Parser)]
#[command(name = "toastrack", version, about = "Multiboot USB sticks for UEFI, with Secure Boot")]
enum Cmd {
    /// Show whether ISO images can boot with Secure Boot on, and their boot entries.
    Check { isos: Vec<PathBuf> },
    /// Diagnose an ISO as the boot menu sees it: read it through Toastrack's own exFAT reader from a partition or image
    /// (needs read access to the device, e.g. sudo). Example: toastrack check-volume /dev/sda2 isos/ubuntu.iso
    CheckVolume { volume: PathBuf, path: String },
    /// Show which distro profile an ISO gets and the exact kernel command line each boot entry would use.
    Explain {
        iso: PathBuf,
        /// Path of the ISO on the stick (default: /isos/<file name>).
        #[arg(long)]
        path: Option<String>,
        /// Data partition filesystem.
        #[arg(long, value_enum, default_value = "exfat")]
        fs: FsArg,
        /// Data partition UUID as blkid prints it.
        #[arg(long, default_value = "ABCD-1234")]
        uuid: String,
        /// Byte offset of the ISO in the partition (pretend it is stored contiguously there).
        #[arg(long, default_value_t = 1048576)]
        offset: u64,
        /// Also print the boot config the entries were read from (to share when a distro is not recognised).
        #[arg(long)]
        dump_config: bool,
    },
    /// Describe a Windows ISO (layout, boot manager signature, install.wim size, WIM images); safe to paste into an issue.
    WindowsReport {
        iso: PathBuf,
        /// Write the report to this file as well.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Also print the contents of this small text file from the booting image of boot.wim (e.g.
        /// Windows/System32/winpeshl.ini). Off by default: the report otherwise holds no file contents.
        #[arg(long = "show-file")]
        show_file: Vec<String>,
    },
    /// List disks that could be used as a stick.
    List,
    /// Erase a disk (or image file) and set it up as a Toastrack stick.
    Install {
        /// Disk such as /dev/sdb, or an image file.
        target: PathBuf,
        /// Directory with shimx64.efi, mmx64.efi, grubx64.efi and ENROLL_THIS_KEY_IN_MOKMANAGER.cer (default: the bundle folder next to the program)
        /// (and optionally toastrack-attach.exe for Windows ISOs).
        #[arg(long, conflicts_with = "unsigned_loader")]
        bundle: Option<PathBuf>,
        /// Instead of a bundle: a stick for PCs with Secure Boot OFF. This loader (unsigned is fine) becomes the boot file.
        #[arg(long)]
        unsigned_loader: Option<PathBuf>,
        /// The Windows helper (toastrack-attach.exe) to put on the stick; with --bundle it is taken from the bundle.
        #[arg(long)]
        attach_exe: Option<PathBuf>,
        /// When TARGET is an image file that does not exist yet: create it with this size (e.g. 8G, 512M).
        #[arg(long)]
        size: Option<String>,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
        /// Allow disks that do not look like USB/removable drives.
        #[arg(long)]
        allow_internal: bool,
        /// Filesystem of the ISO partition. exFAT allows files over 4 GB (recommended); FAT32 limits files to 4 GB.
        #[arg(long, value_enum, default_value = "exfat")]
        data_fs: FsArg,
    },
    /// Replace shim, loader and key on an existing stick without touching the ISOs.
    Update {
        target: PathBuf,
        #[arg(long, conflicts_with = "unsigned_loader")]
        bundle: Option<PathBuf>,
        #[arg(long)]
        unsigned_loader: Option<PathBuf>,
        #[arg(long)]
        attach_exe: Option<PathBuf>,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        allow_internal: bool,
    },
    /// Read a stick back and compare it with a bundle.
    Verify {
        target: PathBuf,
        #[arg(long, conflicts_with = "unsigned_loader")]
        bundle: Option<PathBuf>,
        #[arg(long)]
        unsigned_loader: Option<PathBuf>,
        #[arg(long)]
        attach_exe: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum FsArg {
    Exfat,
    Fat32,
}

fn load_bundle(bundle: Option<&Path>, unsigned: Option<&Path>, attach: Option<&Path>) -> Result<Bundle, Error> {
    match (bundle, unsigned) {
        (Some(dir), _) => {
            let mut b = Bundle::load(dir)?;
            if let Some(p) = attach {
                b.attach_exe = Some(std::fs::read(p)?);
                b.validate()?;
            }
            Ok(b)
        }
        (None, Some(loader)) => Bundle::unsigned(loader, attach),
        (None, None) => match Bundle::discover() {
            Some(dir) => load_bundle(Some(&dir), None, attach),
            None => Err(Error::Bundle("give --bundle <dir> or --unsigned-loader <file> (no bundle folder found next to the program)".into())),
        },
    }
}

fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (num, mult) = match s.chars().last() {
        Some('G' | 'g') => (&s[..s.len() - 1], 1u64 << 30),
        Some('M' | 'm') => (&s[..s.len() - 1], 1u64 << 20),
        _ => (s, 1),
    };
    num.parse::<u64>().map(|n| n * mult).map_err(|_| format!("bad size {s:?} (try 8G or 512M)"))
}

fn confirm(prompt: &str, expect: &str) -> bool {
    print!("{prompt}\nType {expect:?} to continue: ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).is_ok() && line.trim() == expect
}

fn run(cmd: Cmd) -> Result<ExitCode, Error> {
    match cmd {
        Cmd::Check { isos } => {
            if isos.is_empty() {
                eprintln!("usage: toastrack check <image.iso>...");
                return Ok(ExitCode::from(2));
            }
            let mut ok = true;
            for p in &isos {
                let status = File::open(p)
                    .map_err(|e| e.to_string())
                    .and_then(|f| Iso::open(BufReader::new(f)).map_err(|e| e.to_string()))
                    .and_then(|mut iso| analyze_iso(&mut iso).map_err(|e| e.to_string()));
                let label = match &status {
                    Ok(SecureBootStatus::Trusted { .. }) => "OK       secure-boot ready".to_string(),
                    Ok(SecureBootStatus::NeedsEnrollment { signers }) => format!("ENROLL   needs key enrollment ({})", signers.join("; ")),
                    Ok(SecureBootStatus::Unsigned) => "BLOCKED  unsigned loader".to_string(),
                    Ok(SecureBootStatus::NoUefiLoader) => "UNKNOWN  no BOOTX64.EFI found".to_string(),
                    Err(e) => format!("ERROR    {e}"),
                };
                println!("{label}  {}", p.display());
                if matches!(status, Ok(SecureBootStatus::NoUefiLoader)) {
                    // Say why: the loader file is missing, unreadable, or not an x86-64 PE image.
                    if let Ok(f) = File::open(p) {
                        if let Ok(mut iso) = Iso::open(BufReader::new(f)) {
                            match iso.find("EFI/BOOT/BOOTX64.EFI") {
                                Ok(Some(e)) => match iso.read_file(&e) {
                                    Ok(d) => match toastrack_core::pe::inspect(&d) {
                                        Ok(i) => println!("         loader found ({} bytes) but machine type is {:#x}, not x86-64", d.len(), i.machine),
                                        Err(err) => println!("         loader found ({} bytes) but could not be parsed: {err:?}", d.len()),
                                    },
                                    Err(err) => println!("         loader found but could not be read: {err}"),
                                },
                                Ok(None) => println!("         no EFI/BOOT/BOOTX64.EFI in the ISO 9660 tree, and none in a boot image"),
                                Err(err) => println!("         looking for the loader failed: {err}"),
                            }
                        }
                    }
                }
                if let Ok(f) = File::open(p) {
                    if let Ok(mut iso) = Iso::open(BufReader::new(f)) {
                        for e in toastrack_core::bootcfg::entries(&mut iso).unwrap_or_default() {
                            println!("         boot entry: {:?} kernel={} initrd={:?}", e.title, e.kernel, e.initrds);
                        }
                    }
                }
                ok &= matches!(status, Ok(SecureBootStatus::Trusted { .. }));
            }
            Ok(if ok { ExitCode::SUCCESS } else { ExitCode::from(1) })
        }
        Cmd::CheckVolume { volume, path } => {
            use toastrack_core::{exfat::ExfatVolume, io::{ReadAt, Shared}};
            let src = Shared::new(File::open(&volume)?);
            let mut vol = ExfatVolume::open(src).map_err(|e| Error::Layout(format!("not an exFAT volume: {e:?}")))?;
            let entry = vol.find(&path).map_err(|e| Error::Layout(format!("{e:?}")))?.ok_or_else(|| Error::Layout(format!("{path} not found on the volume")))?;
            let mut f = vol.open_file(&entry);
            println!("file: {} bytes, contiguous: {}", f.size(), f.is_contiguous());
            let runs = f.extents().map(|r| r.len()).map_err(|e| format!("{e:?}"));
            println!("physical runs: {runs:?}");
            let mut head = [0u8; 4];
            f.read_at(32768, &mut head).map_err(|_| Error::Layout("cannot read the ISO header".into()))?;
            println!("bytes at 32768: {:02x?} (expect 01 43 44 30 for an ISO 9660 volume descriptor)", head);
            let mut iso = Iso::open(f).map_err(|e| Error::Layout(format!("ISO open: {e}")))?;
            println!("verdict: {:?}", analyze_iso(&mut iso).map_err(|e| e.to_string()));
            match iso.find("EFI/BOOT/BOOTX64.EFI") {
                Ok(Some(e)) => match iso.read_file(&e) {
                    Ok(d) => println!("loader {} bytes, starts {:02x?}, parse: {:?}", d.len(), &d[..d.len().min(4)], toastrack_core::pe::inspect(&d).map(|i| (i.machine, i.signed, i.signer_subjects))),
                    Err(err) => println!("loader read failed: {err}"),
                },
                other => println!("find: {:?}", other.map(|o| o.map(|e| (e.lba, e.size))).map_err(|e| e.to_string())),
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Explain { iso, path, fs, uuid, offset, dump_config } => {
            use toastrack_core::{bootcfg, profile};
            let mut i = Iso::open(BufReader::new(File::open(&iso)?)).map_err(|e| Error::Layout(e.to_string()))?;
            let Some((cfg_path, cfg_text)) = bootcfg::load(&mut i).map_err(|e| Error::Layout(e.to_string()))? else {
                println!("no GRUB boot config with Linux entries found in this ISO (looked for loopback.cfg and grub.cfg)");
                return Ok(ExitCode::from(1));
            };
            let name = iso.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let placement = profile::Placement {
                path: path.unwrap_or_else(|| format!("/isos/{name}")),
                fs_uuid: uuid,
                fs: match fs { FsArg::Exfat => profile::Fs::Exfat, FsArg::Fat32 => profile::Fs::Fat },
                raw_offset: Some(offset),
                fragments: 1,
            };
            println!("config: {cfg_path}   placement: {} on {:?}", placement.path, placement.fs);
            let mut ok = true;
            for e in bootcfg::parse(&cfg_text) {
                println!("\n* {:?}\n  kernel {}   initrd {:?}", e.title, e.kernel, e.initrds);
                match profile::plan(&e, &placement) {
                    Ok(p) => {
                        println!("  profile: {} ({:?})", p.family.name(), p.family.status());
                        println!("  cmdline: {}", p.cmdline);
                        for n in p.notes {
                            println!("  note: {n}");
                        }
                    }
                    Err(err) => {
                        ok = false;
                        println!("  cannot boot: {err}");
                    }
                }
            }
            if dump_config {
                println!("\n----- {cfg_path} -----\n{cfg_text}");
            }
            Ok(if ok { ExitCode::SUCCESS } else { ExitCode::from(1) })
        }
        Cmd::WindowsReport { iso, out, show_file } => match winreport::report(&iso, &show_file) {
            Ok(text) => {
                print!("{text}");
                if let Some(p) = out {
                    std::fs::write(p, &text)?;
                }
                Ok(ExitCode::SUCCESS)
            }
            Err(e) => {
                eprintln!("error: {e}");
                Ok(ExitCode::from(1))
            }
        },
        Cmd::List => {
            let disks = device::list();
            if disks.is_empty() {
                println!("no disks found (listing is implemented on Linux and Windows)");
            }
            for d in disks {
                println!("{}", d.describe());
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Install { target, bundle, unsigned_loader, attach_exe, size, yes, allow_internal, data_fs } => {
            let bundle = load_bundle(bundle.as_deref(), unsigned_loader.as_deref(), attach_exe.as_deref())?;
            let size = size.map(|s| parse_size(&s)).transpose().map_err(Error::Layout)?;
            let exists = target.exists();
            if !exists && size.is_none() {
                return Err(Error::Layout(format!("{} does not exist; to create an image file pass --size", target.display())));
            }
            let disk = device::check_target(&target, allow_internal)?;
            let what = match &disk {
                Some(d) => format!("ALL DATA on {} will be erased.", d.describe()),
                None if exists => format!("ALL DATA in the image file {} will be erased.", target.display()),
                None => format!("A new image file {} will be created.", target.display()),
            };
            if !yes && !(disk.is_none() && !exists) {
                let expect = disk.as_ref().map(|d| d.path.display().to_string()).unwrap_or_else(|| "yes".into());
                if !confirm(&what, &expect) {
                    eprintln!("aborted");
                    return Ok(ExitCode::from(1));
                }
            }
            let parts = ops::install_to(&target, &bundle, match data_fs { FsArg::Exfat => install::DataFs::Exfat, FsArg::Fat32 => install::DataFs::Fat32 }, size, allow_internal)?;
            println!("installed on {}", target.display());
            println!("  EFI partition  at {} MiB", parts.esp.0 >> 20);
            println!("  data partition at {} MiB ({} MiB)", parts.data.0 >> 20, parts.data.1 >> 20);
            for line in ops::next_steps(&bundle) {
                println!("{line}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Update { target, bundle, unsigned_loader, attach_exe, yes, allow_internal } => {
            let bundle = load_bundle(bundle.as_deref(), unsigned_loader.as_deref(), attach_exe.as_deref())?;
            device::check_target(&target, allow_internal)?;
            if !yes && !confirm(&format!("The boot files on {} will be replaced (your ISOs stay).", target.display()), "yes") {
                eprintln!("aborted");
                return Ok(ExitCode::from(1));
            }
            ops::update_at(&target, &bundle, allow_internal)?;
            println!("updated {}", target.display());
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Verify { target, bundle, unsigned_loader, attach_exe } => {
            let bundle = load_bundle(bundle.as_deref(), unsigned_loader.as_deref(), attach_exe.as_deref())?;
            let problems = ops::verify_at(&target, &bundle)?;
            if problems.is_empty() {
                println!("OK: {} matches the bundle", target.display());
                Ok(ExitCode::SUCCESS)
            } else {
                for p in &problems {
                    println!("PROBLEM: {p}");
                }
                Ok(ExitCode::from(1))
            }
        }
    }
}

fn main() -> ExitCode {
    match run(Cmd::parse()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}
