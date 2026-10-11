//! Extract Linux boot entries from an ISO's GRUB loopback config.
//!
//! Most Linux live ISOs ship `/boot/grub/loopback.cfg`, written for GRUB's `loopback` command.
//! It uses `${iso_path}` as the place where the ISO's own path goes, e.g.
//! `linux /casper/vmlinuz iso-scan/filename=${iso_path} ---`.
//! That is exactly the information needed to start the kernel directly and have the
//! initramfs find the ISO on the stick.

use crate::{io::ReadAt, iso::{Iso, IsoError}};
use alloc::{string::{String, ToString}, vec::Vec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootEntry {
    pub title: String,
    pub kernel: String,
    pub initrds: Vec<String>,
    /// Kernel command line with `${iso_path}` still unexpanded.
    pub args: String,
}

/// Where the ISO file is, as the booted Linux must be told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IsoLocation {
    /// A path on a filesystem Linux can mount (FAT32): the initramfs searches for it (`iso-scan/filename`).
    Path(String),
    /// A contiguous byte range of a partition whose filesystem Linux's initramfs cannot mount (exFAT):
    /// handed over as a raw offset (`live-media` + `LIVEMEDIA_OFFSET`, casper-based ISOs only; casper reads the offset
    /// from the environment, which the kernel fills from unknown `NAME=value` boot arguments).
    Partition { uuid: String, offset: u64 },
}

impl BootEntry {
    /// Kernel command line for `iso_path` (see [`IsoLocation::Path`]).
    pub fn cmdline(&self, iso_path: &str) -> String {
        self.args.replace("${iso_path}", iso_path).replace("$iso_path", iso_path)
    }

    /// Ubuntu-family ISOs (casper) are the ones that understand `LIVEMEDIA_OFFSET`.
    pub fn is_casper(&self) -> bool {
        self.kernel.starts_with("/casper/") || self.args.split_whitespace().any(|a| a == "boot=casper")
    }

    pub fn cmdline_for(&self, loc: &IsoLocation) -> Result<String, String> {
        match loc {
            IsoLocation::Path(p) => Ok(self.cmdline(p)),
            IsoLocation::Partition { uuid, offset } => {
                if !self.is_casper() {
                    return Err(String::from(
                        "this ISO cannot be booted from an exFAT partition yet (only Ubuntu-family ISOs can); \
                         put it on a FAT32 stick, if it is under 4 GB",
                    ));
                }
                // Drop the filename search, add the raw location before the `---` separator.
                let kept: Vec<&str> = self
                    .args
                    .split_whitespace()
                    .filter(|a| !a.starts_with("iso-scan/filename="))
                    .collect();
                let extra = alloc::format!("live-media=/dev/disk/by-uuid/{uuid} LIVEMEDIA_OFFSET={offset}");
                let mut out = Vec::new();
                let mut inserted = false;
                for a in kept {
                    if a == "---" && !inserted {
                        out.push(extra.as_str());
                        inserted = true;
                    }
                    out.push(a);
                }
                if !inserted {
                    out.push(extra.as_str());
                }
                Ok(out.join(" "))
            }
        }
    }
}

/// A minimal `grub.cfg` that boots exactly one entry immediately.
/// Served in place of the ISO's own config so the distro's signed GRUB starts the kernel
/// (and does the Secure Boot verification) with `iso-scan/filename` filled in.
pub fn single_entry_config(e: &BootEntry, loc: &IsoLocation) -> Result<String, String> {
    let cmdline = e.cmdline_for(loc)?;
    let mut c = String::from("set timeout=0\nset default=0\n");
    c.push_str("menuentry \"");
    c.push_str(&e.title.replace('"', ""));
    c.push_str("\" {\n set gfxpayload=keep\n linux ");
    c.push_str(&e.kernel);
    c.push(' ');
    c.push_str(&cmdline);
    c.push('\n');
    if !e.initrds.is_empty() {
        c.push_str(" initrd");
        for i in &e.initrds {
            c.push(' ');
            c.push_str(i);
        }
        c.push('\n');
    }
    c.push_str("}\n");
    Ok(c)
}

const CONFIGS: [&str; 3] = ["boot/grub/loopback.cfg", "boot/grub/grub.cfg", "EFI/boot/grub.cfg"];

fn unquote(s: &str) -> &str {
    s.trim().trim_matches(|c| c == '"' || c == '\'')
}

pub fn parse(cfg: &str) -> Vec<BootEntry> {
    let mut out = Vec::new();
    let mut cur: Option<BootEntry> = None;
    for line in cfg.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("menuentry") {
            if let Some(e) = cur.take() {
                push(&mut out, e);
            }
            let title = rest.split_once('{').map_or(rest, |(t, _)| t);
            // Title is the first quoted string.
            let title = title.trim();
            let q = title.chars().next().filter(|c| *c == '"' || *c == '\'');
            let title = match q {
                Some(q) => title[1..].split(q).next().unwrap_or(""),
                None => title.split_whitespace().next().unwrap_or(""),
            };
            cur = Some(BootEntry { title: title.to_string(), kernel: String::new(), initrds: Vec::new(), args: String::new() });
        } else if let Some(e) = cur.as_mut() {
            let mut w = line.splitn(2, char::is_whitespace);
            match (w.next(), w.next()) {
                (Some("linux" | "linuxefi"), Some(rest)) if e.kernel.is_empty() => {
                    let rest = rest.trim();
                    let (k, a) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                    e.kernel = k.to_string();
                    e.args = a.trim().to_string();
                }
                (Some("initrd" | "initrdefi"), Some(rest)) => {
                    e.initrds.extend(rest.split_whitespace().map(|s| unquote(s).to_string()));
                }
                _ => {}
            }
        }
    }
    if let Some(e) = cur {
        push(&mut out, e);
    }
    out
}

fn push(out: &mut Vec<BootEntry>, e: BootEntry) {
    // Only entries that start a kernel are usable; also skip memtest and similar BIOS-only items.
    if !e.kernel.is_empty() && !e.kernel.contains("memtest") {
        out.push(e);
    }
}

/// Find the first usable config on the ISO and parse it.
/// Plain `grub.cfg` is only used if it contains `${iso_path}`-style entries or a loopback config is absent.
pub fn entries<R: ReadAt>(iso: &mut Iso<R>) -> Result<Vec<BootEntry>, IsoError> {
    for path in CONFIGS {
        if let Some(f) = iso.find(path)? {
            if f.size == 0 || f.size > 1 << 20 {
                continue;
            }
            let data = iso.read_file(&f)?;
            let text = String::from_utf8_lossy(&data);
            let found = parse(&text);
            if !found.is_empty() {
                return Ok(found);
            }
        }
    }
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    const UBUNTU: &str = r#"
menuentry "Try or Install Ubuntu Server" {
	set gfxpayload=keep
	linux	/casper/vmlinuz  iso-scan/filename=${iso_path} ---
	initrd	/casper/initrd
}
menuentry "HWE" {
	linux	/casper/hwe-vmlinuz  iso-scan/filename=${iso_path} ---
	initrd	/casper/hwe-initrd
}
grub_platform
menuentry 'Test memory' {
	linux16 /boot/memtest86+x64.bin
}
"#;

    #[test]
    fn single_entry_config_substitutes_iso_path() {
        let e = &parse(UBUNTU)[0];
        let c = single_entry_config(e, &IsoLocation::Path("/isos/u.iso".into())).unwrap();
        assert!(c.contains("linux /casper/vmlinuz iso-scan/filename=/isos/u.iso ---"));
        assert!(c.contains("initrd /casper/initrd"));
        assert!(c.starts_with("set timeout=0"));
    }

    #[test]
    fn exfat_location_replaces_iso_scan_with_raw_offset() {
        let e = &parse(UBUNTU)[0];
        let loc = IsoLocation::Partition { uuid: "CAFE-F00D".into(), offset: 123456 };
        assert_eq!(
            e.cmdline_for(&loc).unwrap(),
            "live-media=/dev/disk/by-uuid/CAFE-F00D LIVEMEDIA_OFFSET=123456 ---"
        );
        let c = single_entry_config(e, &loc).unwrap();
        assert!(!c.contains("iso-scan"));
    }

    #[test]
    fn exfat_location_refused_for_non_casper_isos() {
        let cfg = "menuentry \"Debian\" {\n linux /live/vmlinuz boot=live findiso=${iso_path}\n initrd /live/initrd.img\n}\n";
        let e = &parse(cfg)[0];
        let err = e.cmdline_for(&IsoLocation::Partition { uuid: "AAAA-BBBB".into(), offset: 1 }).unwrap_err();
        assert!(err.contains("exFAT"));
        assert!(e.cmdline_for(&IsoLocation::Path("/x.iso".into())).is_ok());
    }

    #[test]
    fn parses_ubuntu_loopback() {
        let e = parse(UBUNTU);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].title, "Try or Install Ubuntu Server");
        assert_eq!(e[0].kernel, "/casper/vmlinuz");
        assert_eq!(e[0].initrds, ["/casper/initrd"]);
        assert_eq!(e[0].cmdline("/isos/u.iso"), "iso-scan/filename=/isos/u.iso ---");
    }
}
