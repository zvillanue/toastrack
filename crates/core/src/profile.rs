//! Distro profiles: how each family of live/installer ISOs is told where its own ISO file is.
//!
//! A booted initramfs must locate the ISO again (the firmware's view of it is gone). Every family has its
//! own kernel parameters for that; this module detects the family from a boot entry, removes whatever
//! location parameters the entry already carries, and adds the right ones.
//!
//! Verification status is part of the data (see [`Family::status`]): only Ubuntu-family (casper) has been
//! booted for real; the others follow the families' own initramfs sources or documentation.

use crate::bootcfg::BootEntry;
use alloc::{format, string::{String, ToString}, vec::Vec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Ubuntu and derivatives (casper).
    Casper,
    /// Debian live, Kali, Tails and others using live-boot.
    DebianLive,
    /// Fedora/RHEL-style live images (dracut dmsquash-live).
    Dracut,
    /// Fedora/RHEL/Alma/Rocky installer media (anaconda).
    Anaconda,
    /// Arch Linux, Manjaro, EndeavourOS and others using archiso/miso hooks.
    Archiso,
    /// Not recognised; only GRUB variables in the entry can be filled in.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Booted for real (in the project's QEMU setup), in each way listed in docs/DESIGN.md.
    BootTested,
    /// Parameters checked against the family's initramfs source code; not booted.
    SourceChecked,
    /// Parameters taken from the family's documentation; not booted.
    DocsOnly,
    Unknown,
}

impl Family {
    pub fn name(self) -> &'static str {
        match self {
            Family::Casper => "Ubuntu family (casper)",
            Family::DebianLive => "Debian live (live-boot)",
            Family::Dracut => "Fedora/RHEL live (dracut)",
            Family::Anaconda => "Fedora/RHEL installer (anaconda)",
            Family::Archiso => "Arch family (archiso)",
            Family::Other => "unrecognised",
        }
    }

    pub fn status(self) -> Status {
        match self {
            Family::Casper => Status::BootTested,
            Family::DebianLive | Family::Dracut => Status::SourceChecked,
            Family::Anaconda | Family::Archiso => Status::DocsOnly,
            Family::Other => Status::Unknown,
        }
    }
}

fn has_prefix(tokens: &[&str], p: &str) -> bool {
    tokens.iter().any(|t| t.starts_with(p))
}

/// Decide the family from the entry's kernel path and command line.
pub fn detect(e: &BootEntry) -> Family {
    let t: Vec<&str> = e.args.split_whitespace().collect();
    let is = |x: &str| t.contains(&x);
    if ["archisobasedir=", "archisolabel=", "archisosearchuuid=", "misobasedir=", "misolabel="].iter().any(|p| has_prefix(&t, p)) {
        Family::Archiso
    } else if has_prefix(&t, "inst.stage2=") || has_prefix(&t, "inst.repo=") {
        Family::Anaconda
    } else if has_prefix(&t, "root=live:") || is("rd.live.image") {
        Family::Dracut
    } else if is("boot=casper") || e.kernel.starts_with("/casper/") {
        Family::Casper
    } else if is("boot=live") || e.kernel.starts_with("/live/") {
        Family::DebianLive
    } else {
        Family::Other
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fs {
    /// FAT12/16/32: every Linux initramfs can mount it.
    Fat,
    /// exFAT: most initramfs images cannot mount it.
    Exfat,
}

/// Where the ISO file is on the stick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// Path on the data volume, e.g. `/isos/ubuntu.iso`.
    pub path: String,
    /// The data volume's UUID as blkid prints it (`ABCD-1234`).
    pub fs_uuid: String,
    pub fs: Fs,
    /// Byte offset of the ISO within the data partition, if the file is stored contiguously.
    pub raw_offset: Option<u64>,
    /// How many separate pieces the file is stored in (1 = contiguous).
    pub fragments: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub family: Family,
    pub cmdline: String,
    /// Caveats to show the user (untested family, relies on the ISO's own exFAT support, ...).
    pub notes: Vec<String>,
}

fn dev_path(uuid: &str) -> String {
    format!("/dev/disk/by-uuid/{uuid}")
}

/// Expand GRUB-style variables in `s` the way loopback configs use them. Unknown ones expand to nothing,
/// as in GRUB; their names go to `unknown`.
fn expand(s: &str, p: &Placement, unknown: &mut Vec<String>, used: &mut bool) -> String {
    let b = s.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'$' {
            let ch = s[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        let braced = b.get(i + 1) == Some(&b'{');
        let start = i + 1 + braced as usize;
        let mut end = start;
        while end < b.len() && (b[end].is_ascii_alphanumeric() || b[end] == b'_') {
            end += 1;
        }
        let name = &s[start..end];
        if name.is_empty() || (braced && b.get(end) != Some(&b'}')) {
            out.push('$');
            i += 1;
            continue;
        }
        match name {
            "iso_path" | "isopath" | "isofile" | "iso_file" => {
                out.push_str(&p.path);
                *used = true;
            }
            "imgdevpath" => {
                out.push_str(&dev_path(&p.fs_uuid));
                *used = true;
            }
            "imgdevuuid" | "rootuuid" | "isodevuuid" => {
                out.push_str(&p.fs_uuid);
                *used = true;
            }
            other => unknown.push(other.to_string()),
        }
        i = end + braced as usize;
    }
    out
}

fn insert_before_separator(tokens: &mut Vec<String>, extra: &[String]) {
    let at = tokens.iter().position(|t| t == "---").unwrap_or(tokens.len());
    for (k, e) in extra.iter().enumerate() {
        tokens.insert(at + k, e.clone());
    }
}

/// Replace the first token starting with `prefix` (dropping any further ones), or insert before `---`.
fn replace_or_insert(tokens: &mut Vec<String>, prefix: &str, value: String) {
    match tokens.iter().position(|t| t.starts_with(prefix)) {
        Some(i) => {
            tokens[i] = value;
            let mut seen = false;
            tokens.retain(|t| !t.starts_with(prefix) || !core::mem::replace(&mut seen, true));
        }
        None => insert_before_separator(tokens, &[value]),
    }
}

fn drop_prefixes(tokens: &mut Vec<String>, prefixes: &[&str]) {
    tokens.retain(|t| !prefixes.iter().any(|p| t.starts_with(p)));
}

const FRAGMENTED: &str = "the ISO file is stored in separate pieces on the exFAT partition; this distro can only be booted from a contiguous file. Delete it and copy it again (an emptier stick helps)";

/// Work out the kernel command line for `e` with the ISO at `p`.
pub fn plan(e: &BootEntry, p: &Placement) -> Result<Plan, String> {
    let family = detect(e);
    let needs_uuid = p.fs == Fs::Exfat || matches!(family, Family::Anaconda | Family::Archiso) || e.args.contains("imgdev") || e.args.contains("uuid");
    if needs_uuid && p.fs_uuid.is_empty() {
        return Err(String::from("cannot determine the data volume's UUID, which this ISO needs to find its own file"));
    }
    let (mut unknown, mut used) = (Vec::new(), false);
    let mut tokens: Vec<String> =
        e.args.split_whitespace().map(|t| expand(t, p, &mut unknown, &mut used)).filter(|t| !t.is_empty()).collect();
    let mut notes: Vec<String> = Vec::new();
    if !unknown.is_empty() {
        notes.push(format!("ignored unknown GRUB variables: {}", unknown.join(", ")));
    }
    let raw = |what: &str| -> Result<u64, String> {
        p.raw_offset.ok_or_else(|| format!("{what}: {FRAGMENTED} ({} pieces)", p.fragments))
    };
    let exfat_note = "exFAT: relies on this ISO's own initramfs being able to mount exFAT (untested)";
    match family {
        Family::Casper => {
            drop_prefixes(&mut tokens, &["iso-scan/filename=", "live-media=", "LIVEMEDIA_OFFSET="]);
            let extra = match p.fs {
                Fs::Fat => alloc::vec![format!("iso-scan/filename={}", p.path)],
                // Ubuntu's initramfs has no exFAT driver, so give it the raw location instead; casper reads
                // LIVEMEDIA_OFFSET from the environment, which the kernel fills from `NAME=value` arguments.
                Fs::Exfat => alloc::vec![format!("live-media={}", dev_path(&p.fs_uuid)), format!("LIVEMEDIA_OFFSET={}", raw("casper")?)],
            };
            insert_before_separator(&mut tokens, &extra);
        }
        Family::DebianLive => {
            drop_prefixes(&mut tokens, &["findiso=", "fromiso=", "live-media=", "bootfrom=", "live-media-offset="]);
            let extra = match p.fs {
                Fs::Fat => alloc::vec![format!("findiso={}", p.path)],
                Fs::Exfat => alloc::vec![format!("live-media={}", dev_path(&p.fs_uuid)), format!("live-media-offset={}", raw("live-boot")?)],
            };
            insert_before_separator(&mut tokens, &extra);
        }
        Family::Dracut => {
            drop_prefixes(&mut tokens, &["iso-scan/filename="]);
            insert_before_separator(&mut tokens, &[format!("iso-scan/filename={}", p.path)]);
            if p.fs == Fs::Exfat {
                notes.push(exfat_note.to_string());
            }
        }
        Family::Anaconda => {
            replace_or_insert(&mut tokens, "inst.stage2=", format!("inst.stage2=hd:UUID={}:{}", p.fs_uuid, p.path));
            if p.fs == Fs::Exfat {
                notes.push(exfat_note.to_string());
            }
        }
        Family::Archiso => {
            drop_prefixes(&mut tokens, &["archisosearchuuid=", "archisolabel=", "archisodevice=", "misosearchuuid=", "misolabel=", "misodevice=", "img_dev=", "img_loop="]);
            insert_before_separator(&mut tokens, &[format!("img_dev={}", dev_path(&p.fs_uuid)), format!("img_loop={}", p.path)]);
            if p.fs == Fs::Exfat {
                notes.push(exfat_note.to_string());
            }
        }
        Family::Other => {
            if !used {
                return Err(String::from(
                    "no boot profile for this ISO and its boot entry has no place to put the ISO's location; \
                     use \"Boot the ISO's own bootloader\" instead",
                ));
            }
        }
    }
    match family.status() {
        Status::BootTested => {}
        Status::SourceChecked => notes.push(format!("{}: checked against the distro's initramfs source, not boot-tested", family.name())),
        Status::DocsOnly => notes.push(format!("{}: based on documentation, not boot-tested", family.name())),
        Status::Unknown => notes.push(String::from("unrecognised distro: only the entry's own placeholders were filled in")),
    }
    Ok(Plan { family, cmdline: tokens.join(" "), notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootcfg::parse;

    // Entries below are representative of each family's boot configs (modelled on upstream layouts), not
    // copied from real ISOs, except the Ubuntu one which is from the real 24.04 server ISO.
    fn entry(linux_args: &str, kernel: &str) -> BootEntry {
        parse(&alloc::format!("menuentry \"T\" {{\n linux {kernel} {linux_args}\n initrd /i\n}}\n")).remove(0)
    }

    fn fat() -> Placement {
        Placement { path: "/isos/x.iso".into(), fs_uuid: "ABCD-1234".into(), fs: Fs::Fat, raw_offset: None, fragments: 1 }
    }

    fn exfat(off: Option<u64>) -> Placement {
        Placement { path: "/isos/x.iso".into(), fs_uuid: "CAFE-F00D".into(), fs: Fs::Exfat, raw_offset: off, fragments: if off.is_some() { 1 } else { 3 } }
    }

    #[test]
    fn detects_families() {
        assert_eq!(detect(&entry("iso-scan/filename=${iso_path} ---", "/casper/vmlinuz")), Family::Casper);
        assert_eq!(detect(&entry("boot=live components findiso=${iso_path}", "/live/vmlinuz-6.1")), Family::DebianLive);
        assert_eq!(detect(&entry("root=live:CDLABEL=Fedora-WS-Live rd.live.image quiet", "/images/pxeboot/vmlinuz")), Family::Dracut);
        assert_eq!(detect(&entry("inst.stage2=hd:LABEL=Fedora-S-dvd quiet", "/images/pxeboot/vmlinuz")), Family::Anaconda);
        assert_eq!(detect(&entry("archisobasedir=arch archisolabel=ARCH_202401", "/arch/boot/x86_64/vmlinuz-linux")), Family::Archiso);
        assert_eq!(detect(&entry("quiet", "/boot/vmlinuz")), Family::Other);
    }

    #[test]
    fn casper_fat_and_exfat() {
        let e = entry("iso-scan/filename=${iso_path} ---", "/casper/vmlinuz");
        assert_eq!(plan(&e, &fat()).unwrap().cmdline, "iso-scan/filename=/isos/x.iso ---");
        let p = plan(&e, &exfat(Some(4096))).unwrap();
        assert_eq!(p.cmdline, "live-media=/dev/disk/by-uuid/CAFE-F00D LIVEMEDIA_OFFSET=4096 ---");
        assert!(p.notes.is_empty());
        let err = plan(&e, &exfat(None)).unwrap_err();
        assert!(err.contains("separate pieces") && err.contains("3 pieces"), "{err}");
    }

    #[test]
    fn casper_without_loopback_placeholder_still_gets_location() {
        // grub.cfg-style entry that has no iso-scan parameter at all.
        let e = entry("boot=casper quiet splash ---", "/casper/vmlinuz");
        assert_eq!(plan(&e, &fat()).unwrap().cmdline, "boot=casper quiet splash iso-scan/filename=/isos/x.iso ---");
    }

    #[test]
    fn debian_live_uses_findiso_or_media_offset() {
        let e = entry("boot=live components quiet splash findiso=${iso_path}", "/live/vmlinuz-6.1.0-18-amd64");
        assert_eq!(plan(&e, &fat()).unwrap().cmdline, "boot=live components quiet splash findiso=/isos/x.iso");
        let e = entry("boot=live components quiet splash", "/live/vmlinuz");
        assert_eq!(
            plan(&e, &exfat(Some(1 << 30))).unwrap().cmdline,
            "boot=live components quiet splash live-media=/dev/disk/by-uuid/CAFE-F00D live-media-offset=1073741824"
        );
        assert!(plan(&e, &fat()).unwrap().notes.iter().any(|n| n.contains("not boot-tested")));
    }

    #[test]
    fn dracut_live_adds_iso_scan_and_keeps_label() {
        let e = entry("root=live:CDLABEL=Fedora-WS-Live-39-1-5 rd.live.image quiet", "/images/pxeboot/vmlinuz");
        let p = plan(&e, &fat()).unwrap();
        assert_eq!(p.cmdline, "root=live:CDLABEL=Fedora-WS-Live-39-1-5 rd.live.image quiet iso-scan/filename=/isos/x.iso");
        assert!(plan(&e, &exfat(None)).unwrap().notes.iter().any(|n| n.contains("exFAT")));
    }

    #[test]
    fn anaconda_points_stage2_at_the_iso_file() {
        let e = entry("inst.stage2=hd:LABEL=Fedora-S-dvd-x86_64-39 quiet", "/images/pxeboot/vmlinuz");
        assert_eq!(plan(&e, &fat()).unwrap().cmdline, "inst.stage2=hd:UUID=ABCD-1234:/isos/x.iso quiet");
    }

    #[test]
    fn archiso_replaces_search_params_with_img_dev_and_loop() {
        let e = entry("archisobasedir=arch archisosearchuuid=2024-01-01-00-00-00-00 img_dev=$imgdevpath img_loop=$iso_path", "/arch/boot/x86_64/vmlinuz-linux");
        assert_eq!(
            plan(&e, &fat()).unwrap().cmdline,
            "archisobasedir=arch img_dev=/dev/disk/by-uuid/ABCD-1234 img_loop=/isos/x.iso"
        );
    }

    #[test]
    fn unknown_distro_needs_a_placeholder_and_unknown_variables_vanish() {
        let e = entry("quiet", "/boot/vmlinuz");
        assert!(plan(&e, &fat()).unwrap_err().contains("no boot profile"));
        let e = entry("quiet splash $vt_handoff foo=${iso_path}", "/boot/vmlinuz");
        let p = plan(&e, &fat()).unwrap();
        assert_eq!(p.cmdline, "quiet splash  foo=/isos/x.iso".split_whitespace().collect::<Vec<_>>().join(" "));
        assert!(p.notes.iter().any(|n| n.contains("vt_handoff")));
    }

    #[test]
    fn dollar_signs_that_are_not_variables_survive() {
        let e = entry("root=live:CDLABEL=A$ rd.live.image", "/x/vmlinuz");
        assert!(plan(&e, &fat()).unwrap().cmdline.contains("CDLABEL=A$"));
    }
}
