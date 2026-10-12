//! Partition layout of a Toastrack stick.

use crate::{Error, Result};

pub const SECTOR: u64 = 512;
pub const MIB: u64 = 1024 * 1024;
pub const ALIGN: u64 = MIB;
/// EFI System Partition: shim, our loader, MokManager, the key to enroll.
pub const ESP_BYTES: u64 = 64 * MIB;
/// Smallest data partition worth creating.
pub const MIN_DATA_BYTES: u64 = 256 * MIB;
/// GPT needs room for the primary table (before the first partition) and the backup (end of disk).
const GPT_BACKUP_RESERVE: u64 = MIB;

pub const ESP_NAME: &str = "TOASTRACK-ESP";
pub const DATA_NAME: &str = "TOASTRACK";
/// FAT volume labels (11 bytes max). The loader finds its data volume by `DATA_LABEL`.
pub const ESP_LABEL: &[u8; 11] = b"TSTRK-ESP  ";
pub const DATA_LABEL: &[u8; 11] = b"TOASTRACK  ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub esp_bytes: u64,
    pub data_bytes: u64,
}

impl Layout {
    pub fn for_disk(disk_bytes: u64) -> Result<Self> {
        let overhead = ALIGN + ESP_BYTES + GPT_BACKUP_RESERVE;
        let data = disk_bytes.saturating_sub(overhead) / ALIGN * ALIGN;
        if data < MIN_DATA_BYTES {
            return Err(Error::Layout(format!(
                "disk is too small: {} MiB, need at least {} MiB",
                disk_bytes / MIB,
                (overhead + MIN_DATA_BYTES) / MIB
            )));
        }
        Ok(Self { esp_bytes: ESP_BYTES, data_bytes: data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_tiny_disks() {
        assert!(Layout::for_disk(200 * MIB).is_err());
    }

    #[test]
    fn data_partition_gets_the_rest_aligned() {
        let l = Layout::for_disk(8 * 1024 * MIB).unwrap();
        assert_eq!(l.esp_bytes, 64 * MIB);
        assert_eq!(l.data_bytes % ALIGN, 0);
        assert!(l.esp_bytes + l.data_bytes + 2 * MIB <= 8 * 1024 * MIB);
        assert!(l.data_bytes > 8000 * MIB - 70 * MIB);
    }
}
