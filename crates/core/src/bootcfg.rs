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
    /// Kernel command line exactly as written in the config (GRUB variables unexpanded).
    pub args: String,
    /// Lines from the entry that decide *where* GRUB looks for files (`search`, `set root=`, `insmod`);
    /// without them the kernel path would be resolved on the wrong device.
    pub preamble: Vec<String>,
}

/// A minimal `grub.cfg` that boots exactly one entry immediately, with the command line the profile chose.
/// Served in place of the ISO's own config so the distro's signed GRUB starts the kernel
/// (and does the Secure Boot verification) with the location of the ISO filled in.
pub fn single_entry_config(e: &BootEntry, cmdline: &str) -> String {
    let mut c = String::from("set timeout=0\nset default=0\n");
    c.push_str("menuentry \"");
    c.push_str(&e.title.replace('"', ""));
    c.push_str("\" {\n set gfxpayload=keep\n");
    for l in &e.preamble {
        c.push(' ');
        c.push_str(l);
        c.push('\n');
    }
    c.push_str(" linux ");
    c.push_str(&e.kernel);
    c.push(' ');
    c.push_str(cmdline);
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
    c
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
            cur = Some(BootEntry { title: title.to_string(), kernel: String::new(), initrds: Vec::new(), args: String::new(), preamble: Vec::new() });
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
                // Device selection that must precede the linux line.
                (Some("insmod" | "search"), _) if e.kernel.is_empty() => e.preamble.push(line.to_string()),
                (Some("set"), Some(rest)) if e.kernel.is_empty() && rest.trim_start().starts_with("root=") => e.preamble.push(line.to_string()),
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

/// Find the first config on the ISO that contains bootable entries: (its path, its text).
/// A loopback config is preferred over the plain `grub.cfg`.
pub fn load<R: ReadAt>(iso: &mut Iso<R>) -> Result<Option<(String, String)>, IsoError> {
    for path in CONFIGS {
        if let Some(f) = iso.find(path)? {
            if f.size == 0 || f.size > 1 << 20 {
                continue;
            }
            let data = iso.read_file(&f)?;
            let text = String::from_utf8_lossy(&data).into_owned();
            if !parse(&text).is_empty() {
                return Ok(Some((path.to_string(), text)));
            }
        }
    }
    Ok(None)
}

pub fn entries<R: ReadAt>(iso: &mut Iso<R>) -> Result<Vec<BootEntry>, IsoError> {
    Ok(load(iso)?.map(|(_, text)| parse(&text)).unwrap_or_default())
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
        assert_eq!(e[0].args, "iso-scan/filename=${iso_path} ---");
    }

    #[test]
    fn keeps_device_selection_lines_before_linux() {
        let cfg = "menuentry 'Fedora' {\n insmod iso9660\n search --no-floppy --set=root -l 'Fedora-Live'\n linux /images/pxeboot/vmlinuz root=live:CDLABEL=Fedora-Live rd.live.image\n initrd /images/pxeboot/initrd.img\n}\n";
        let e = &parse(cfg)[0];
        assert_eq!(e.preamble, ["insmod iso9660", "search --no-floppy --set=root -l 'Fedora-Live'"]);
        let c = single_entry_config(e, "root=live:CDLABEL=Fedora-Live rd.live.image iso-scan/filename=/x.iso");
        assert!(c.contains("search --no-floppy --set=root -l 'Fedora-Live'\n linux /images/pxeboot/vmlinuz root=live"));
        assert!(c.starts_with("set timeout=0"));
    }
}
