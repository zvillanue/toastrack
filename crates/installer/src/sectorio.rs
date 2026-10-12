//! Byte-addressed access on top of a device that only accepts whole, aligned sectors (a Windows physical drive).
//!
//! `SectorIo` turns arbitrary reads and writes into sector-aligned transfers through a 4096-byte-aligned bounce buffer,
//! reading the neighbouring bytes first when a write covers only part of a sector.

use std::io::{self, Read, Seek, SeekFrom, Write};

const CHUNK_SECTORS: u64 = 2048; // 1 MiB with 512-byte sectors
const ALIGN: usize = 4096;

/// A byte buffer whose usable part starts at a 4096-byte boundary.
struct Aligned {
    raw: Vec<u8>,
    start: usize,
    len: usize,
}

impl Aligned {
    fn new(len: usize) -> Self {
        let raw = vec![0u8; len + ALIGN];
        let start = (ALIGN - (raw.as_ptr() as usize % ALIGN)) % ALIGN;
        Aligned { raw, start, len }
    }
    fn bytes(&mut self) -> &mut [u8] {
        &mut self.raw[self.start..self.start + self.len]
    }
}

pub struct SectorIo<F> {
    inner: F,
    sector: u64,
    size: u64,
    pos: u64,
    buf: Aligned,
}

impl<F> std::fmt::Debug for SectorIo<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SectorIo {{ sector: {}, size: {} }}", self.sector, self.size)
    }
}

impl<F: Read + Write + Seek> SectorIo<F> {
    /// `size` is the device length in bytes, `sector` its sector size (a power of two, 512 or 4096).
    pub fn new(inner: F, size: u64, sector: u64) -> Self {
        assert!(sector.is_power_of_two() && sector <= ALIGN as u64);
        SectorIo { inner, sector, size, pos: 0, buf: Aligned::new((CHUNK_SECTORS * sector) as usize) }
    }

    pub fn into_inner(self) -> F {
        self.inner
    }

    pub fn get_mut(&mut self) -> &mut F {
        &mut self.inner
    }

    fn read_sectors(&mut self, lba: u64, count: u64) -> io::Result<()> {
        self.inner.seek(SeekFrom::Start(lba * self.sector))?;
        let n = (count * self.sector) as usize;
        self.inner.read_exact(&mut self.buf.bytes()[..n])
    }

    fn write_sectors(&mut self, lba: u64, count: u64) -> io::Result<()> {
        self.inner.seek(SeekFrom::Start(lba * self.sector))?;
        let n = (count * self.sector) as usize;
        self.inner.write_all(&self.buf.bytes()[..n])
    }
}

impl<F: Read + Write + Seek> Read for SectorIo<F> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.size || out.is_empty() {
            return Ok(0);
        }
        let want = out.len().min((self.size - self.pos) as usize);
        let (first, head) = (self.pos / self.sector, (self.pos % self.sector) as usize);
        let sectors = (head + want).div_ceil(self.sector as usize) as u64;
        let sectors = sectors.min(CHUNK_SECTORS);
        self.read_sectors(first, sectors)?;
        let take = want.min(sectors as usize * self.sector as usize - head);
        out[..take].copy_from_slice(&self.buf.bytes()[head..head + take]);
        self.pos += take as u64;
        Ok(take)
    }
}

impl<F: Read + Write + Seek> Write for SectorIo<F> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        if self.pos >= self.size {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "write past the end of the device"));
        }
        let want = data.len().min((self.size - self.pos) as usize);
        let (first, head) = (self.pos / self.sector, (self.pos % self.sector) as usize);
        let sectors = ((head + want).div_ceil(self.sector as usize) as u64).min(CHUNK_SECTORS);
        let span = sectors as usize * self.sector as usize;
        let take = want.min(span - head);
        // Only partly covered first/last sectors need their old contents.
        let head_partial = head != 0;
        let tail_partial = (head + take) % self.sector as usize != 0;
        if head_partial || tail_partial {
            if sectors == 1 || (head_partial && tail_partial) {
                self.read_sectors(first, sectors)?;
            } else if head_partial {
                self.read_sectors(first, 1)?;
            } else {
                // Only the last sector: read it into place after the leading full sectors are overwritten below.
                let last = sectors - 1;
                self.read_sectors(first + last, 1)?;
                let sec = self.sector as usize;
                let tmp: Vec<u8> = self.buf.bytes()[..sec].to_vec();
                self.buf.bytes()[last as usize * sec..last as usize * sec + sec].copy_from_slice(&tmp);
            }
        }
        self.buf.bytes()[head..head + take].copy_from_slice(&data[..take]);
        self.write_sectors(first, sectors)?;
        self.pos += take as u64;
        Ok(take)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<F: Read + Write + Seek> Seek for SectorIo<F> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let new = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(d) => self.size.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = new.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before the start"))?;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Refuses anything that is not whole-sector, like a Windows raw disk.
    struct Strict {
        inner: Cursor<Vec<u8>>,
        sector: u64,
        log: Vec<(char, u64, usize)>,
    }

    impl Strict {
        fn new(len: usize, sector: u64) -> Self {
            Strict { inner: Cursor::new((0..len).map(|i| (i * 7 + 3) as u8).collect()), sector, log: Vec::new() }
        }
    }
    impl Read for Strict {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            let pos = self.inner.position();
            assert!(pos % self.sector == 0 && b.len() as u64 % self.sector == 0, "unaligned read {pos}+{}", b.len());
            assert!(b.as_ptr() as usize % ALIGN == 0, "unaligned buffer");
            self.log.push(('r', pos, b.len()));
            self.inner.read(b)
        }
    }
    impl Write for Strict {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            let pos = self.inner.position();
            assert!(pos % self.sector == 0 && b.len() as u64 % self.sector == 0, "unaligned write {pos}+{}", b.len());
            assert!(b.as_ptr() as usize % ALIGN == 0, "unaligned buffer");
            self.log.push(('w', pos, b.len()));
            self.inner.write(b)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Seek for Strict {
        fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
            if let SeekFrom::Start(n) = to {
                assert!(n % self.sector == 0, "unaligned seek {n}");
            }
            self.inner.seek(to)
        }
    }

    fn pattern(n: usize, seed: u8) -> Vec<u8> {
        (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
    }

    #[test]
    fn unaligned_writes_and_reads_match_a_plain_buffer() {
        for sector in [512u64, 4096] {
            let len = 3 << 20;
            let mut want: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
            let mut io = SectorIo::new(Strict::new(len, sector), len as u64, sector);
            let mut x = 99u64;
            for k in 0..200 {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let off = (x >> 33) as usize % (len - 1);
                let n = 1 + (x >> 20) as usize % (len - off).min(if k % 17 == 0 { 1_500_000 } else { 9000 });
                let data = pattern(n, k as u8);
                io.seek(SeekFrom::Start(off as u64)).unwrap();
                io.write_all(&data).unwrap();
                want[off..off + n].copy_from_slice(&data);
            }
            let mut got = vec![0u8; len];
            io.seek(SeekFrom::Start(0)).unwrap();
            io.read_exact(&mut got).unwrap();
            assert!(got == want, "sector {sector}: contents differ");
            // odd-sized reads from odd offsets
            for off in [0usize, 1, 511, 512, 513, 4095, 4097, len - 700] {
                let mut b = vec![0u8; 650];
                io.seek(SeekFrom::Start(off as u64)).unwrap();
                io.read_exact(&mut b).unwrap();
                assert_eq!(b, want[off..off + 650], "read at {off}");
            }
        }
    }

    #[test]
    fn whole_sector_writes_do_not_read_first() {
        let mut io = SectorIo::new(Strict::new(1 << 20, 512), 1 << 20, 512);
        io.seek(SeekFrom::Start(512)).unwrap();
        io.write_all(&pattern(1024, 1)).unwrap();
        assert!(io.get_mut().log.iter().all(|(op, _, _)| *op == 'w'), "{:?}", io.get_mut().log);
    }

    #[test]
    fn seek_end_and_bounds() {
        let mut io = SectorIo::new(Strict::new(8192, 512), 8192, 512);
        assert_eq!(io.seek(SeekFrom::End(-512)).unwrap(), 7680);
        let mut b = [0u8; 600];
        assert_eq!(io.read(&mut b).unwrap(), 512);
        assert_eq!(io.read(&mut b).unwrap(), 0);
        assert!(io.write_all(&[1]).is_err());
    }
}
