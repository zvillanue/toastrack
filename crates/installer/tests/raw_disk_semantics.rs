//! A Windows physical drive only accepts whole, aligned sectors. This installs and updates through `SectorIo` over a file
//! that enforces exactly that, for both data filesystems, which is the closest test possible without a real disk.

use std::{
    fs::OpenOptions,
    io::{self, Read, Seek, SeekFrom, Write},
};
use toastrack_installer::{
    bundle::Bundle,
    install::{install_unchecked, update_unchecked, verify, DataFs},
    layout::MIB,
    sectorio::SectorIo,
};

#[derive(Debug)]
struct Strict(std::fs::File);

impl Strict {
    fn check(&mut self, len: usize) {
        let pos = self.0.stream_position().unwrap();
        assert!(pos % 512 == 0 && len % 512 == 0, "unaligned access at {pos} len {len}");
    }
}
impl Read for Strict {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        self.check(b.len());
        self.0.read(b)
    }
}
impl Write for Strict {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.check(b.len());
        self.0.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
impl Seek for Strict {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        if let SeekFrom::Start(n) = to {
            assert!(n % 512 == 0, "unaligned seek {n}");
        }
        self.0.seek(to)
    }
}

fn synthetic(tag: u8) -> Bundle {
    Bundle { shim: vec![tag; 50_000], mok_manager: vec![tag + 1; 30_000], loader: vec![tag + 2; 70_000], cert: vec![tag + 3; 800], attach_exe: Some(vec![tag + 4; 20_000]), secure_boot: true }
}

fn run(fs: DataFs) {
    let dir = tempfile::tempdir().unwrap();
    let f = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(dir.path().join("d.img")).unwrap();
    let size = 400 * MIB;
    f.set_len(size).unwrap();
    let dev = SectorIo::new(Strict(f), size, 512);
    let (dev, _) = install_unchecked(dev, &synthetic(1), fs).unwrap();
    let (dev, problems) = verify(dev, &synthetic(1)).unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    let dev = update_unchecked(dev, &synthetic(5)).unwrap();
    let (_, problems) = verify(dev, &synthetic(5)).unwrap();
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn install_and_update_work_with_whole_sector_io_only_fat32() {
    run(DataFs::Fat32);
}

#[test]
fn install_and_update_work_with_whole_sector_io_only_exfat() {
    run(DataFs::Exfat);
}
