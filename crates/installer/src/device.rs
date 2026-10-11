//! Finding disks and refusing to touch the wrong one.

use crate::{Error, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct DiskInfo {
    pub path: PathBuf,
    pub size: u64,
    pub model: String,
    pub removable: bool,
    pub usb: bool,
    pub mounted: bool,
}

impl DiskInfo {
    pub fn describe(&self) -> String {
        format!(
            "{}  {:.1} GB  {}{}{}",
            self.path.display(),
            self.size as f64 / 1e9,
            if self.model.is_empty() { "(unknown model)" } else { &self.model },
            if self.usb { "  [USB]" } else { "" },
            if self.mounted { "  [MOUNTED]" } else { "" }
        )
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::fs;

    fn read(p: impl AsRef<Path>) -> String {
        fs::read_to_string(p).unwrap_or_default().trim().to_string()
    }

    fn mounted_names() -> Vec<String> {
        read("/proc/mounts")
            .lines()
            .filter_map(|l| l.split_whitespace().next())
            .filter_map(|d| d.strip_prefix("/dev/"))
            .map(str::to_string)
            .collect()
    }

    /// `sdb` is mounted if `sdb`, `sdb1`, `nvme0n1p2` ... appear in /proc/mounts.
    fn is_mounted(disk: &str, mounts: &[String]) -> bool {
        mounts.iter().any(|m| {
            m == disk
                || m.strip_prefix(disk)
                    .is_some_and(|rest| rest.trim_start_matches('p').chars().all(|c| c.is_ascii_digit()) && !rest.is_empty())
        })
    }

    #[cfg(test)]
    pub fn tests_is_mounted(d: &str, m: &[String]) -> bool {
        is_mounted(d, m)
    }

    pub fn info(name: &str) -> Option<DiskInfo> {
        let sys = Path::new("/sys/block").join(name);
        if !sys.exists() {
            return None;
        }
        let sectors: u64 = read(sys.join("size")).parse().ok()?;
        let real = fs::canonicalize(&sys).unwrap_or_default();
        Some(DiskInfo {
            path: Path::new("/dev").join(name),
            size: sectors * 512,
            model: read(sys.join("device/model")),
            removable: read(sys.join("removable")) == "1",
            usb: real.to_string_lossy().contains("/usb"),
            mounted: is_mounted(name, &mounted_names()),
        })
    }

    pub fn list() -> Vec<DiskInfo> {
        let mut v: Vec<DiskInfo> = fs::read_dir("/sys/block")
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| !["loop", "ram", "zram", "dm-", "sr", "md", "nbd"].iter().any(|p| n.starts_with(p)))
            .filter_map(|n| info(&n))
            .filter(|d| d.size > 0)
            .collect();
        v.sort_by(|a, b| a.path.cmp(&b.path));
        v
    }

    pub fn resolve(path: &Path) -> Result<DiskInfo> {
        let real = fs::canonicalize(path)?;
        let name = real.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        if Path::new("/sys/class/block").join(&name).join("partition").exists() {
            return Err(Error::Unsafe(format!("{} is a partition; give the whole disk (e.g. /dev/sdb, not /dev/sdb1)", real.display())));
        }
        info(&name).ok_or_else(|| Error::Unsafe(format!("{} is not a disk I can identify", real.display())))
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::*;
    pub fn list() -> Vec<DiskInfo> {
        Vec::new()
    }
    pub fn resolve(_: &Path) -> Result<DiskInfo> {
        Err(Error::Unsafe("writing to physical disks is only implemented on Linux so far; use an image file".into()))
    }
}

pub use imp::list;

pub fn is_block_device(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        std::fs::metadata(path).map(|m| m.file_type().is_block_device()).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

/// Safety checks before writing to a physical disk. Image files are always allowed.
pub fn check_target(path: &Path, allow_internal: bool) -> Result<Option<DiskInfo>> {
    if !is_block_device(path) {
        return Ok(None);
    }
    let d = imp::resolve(path)?;
    if d.mounted {
        return Err(Error::Unsafe(format!("{} has mounted partitions; unmount them first", d.path.display())));
    }
    if !d.usb && !d.removable && !allow_internal {
        return Err(Error::Unsafe(format!(
            "{} does not look like a USB or removable drive (it may be a system disk); pass --allow-internal if you are sure",
            d.path.display()
        )));
    }
    Ok(Some(d))
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn mount_matching() {
        let m = vec!["sda1".to_string(), "nvme0n1p2".to_string(), "sdb".to_string()];
        assert!(super::imp::tests_is_mounted("sda", &m));
        assert!(super::imp::tests_is_mounted("nvme0n1", &m));
        assert!(super::imp::tests_is_mounted("sdb", &m));
        assert!(!super::imp::tests_is_mounted("sdc", &m));
        assert!(!super::imp::tests_is_mounted("sd", &["sda1".to_string()]));
    }
}
