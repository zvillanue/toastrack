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

impl BootEntry {
    pub fn cmdline(&self, iso_path: &str) -> String {
        self.args.replace("${iso_path}", iso_path).replace("$iso_path", iso_path)
    }
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
    fn parses_ubuntu_loopback() {
        let e = parse(UBUNTU);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].title, "Try or Install Ubuntu Server");
        assert_eq!(e[0].kernel, "/casper/vmlinuz");
        assert_eq!(e[0].initrds, ["/casper/initrd"]);
        assert_eq!(e[0].cmdline("/isos/u.iso"), "iso-scan/filename=/isos/u.iso ---");
    }
}
