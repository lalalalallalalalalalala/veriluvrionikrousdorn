//! USB Mass Storage, Bulk-Only Transport.
//!
//! Поверх xHCI: CBW, данные, CSW и SCSI READ(10) / WRITE(10) / READ CAPACITY.
//! Если суперблок Btrfs на месте, этот диск становится корнем.

use spin::Mutex;
use x86_64::instructions::interrupts;

use super::block::{BlockDevice, BlockError};
use super::xhci::{self, Host};

const CACHE: usize = 16 * 1024;

struct Disk {
    host: Host,
    blocks: u64,
    cache: [u8; CACHE],
    cache_lba: u64,
    cache_ok: bool,
}

static DISK: Mutex<Option<Disk>> = Mutex::new(None);

pub fn claim_root() -> bool {
    let probed = match xhci::probe_msc() {
        Ok(probed) => probed,
        Err(_) => return false,
    };
    if probed.block_size != 512 || probed.blocks == 0 {
        return false;
    }
    crate::serial::write_str("usb-ok\n");
    let mut disk = Disk {
        host: probed.host,
        blocks: probed.blocks,
        cache: [0; CACHE],
        cache_lba: 0,
        cache_ok: false,
    };
    if !disk.looks_like_btrfs() {
        return false;
    }
    *DISK.lock() = Some(disk);
    crate::serial::write_str("usb-root\n");
    true
}

pub fn active() -> bool {
    DISK.lock().is_some()
}

pub fn with_disk<R>(f: impl FnOnce(&mut dyn BlockDevice) -> R) -> Result<R, BlockError> {
    interrupts::without_interrupts(|| {
        let mut guard = DISK.lock();
        let disk = guard.as_mut().ok_or(BlockError::Io)?;
        Ok(f(disk))
    })
}

impl Disk {
    fn looks_like_btrfs(&mut self) -> bool {
        let mut sector = [0u8; 512];
        if self.read_blocks(128, &mut sector).is_err() {
            return false;
        }
        sector.get(64..72) == Some(b"_BHRfS_M")
    }
}

impl BlockDevice for Disk {
    fn block_size(&self) -> u64 {
        512
    }

    fn block_count(&self) -> u64 {
        self.blocks
    }

    fn read_blocks(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if buf.len() % 512 != 0 {
            return Err(BlockError::Io);
        }
        let mut done = 0usize;
        while done < buf.len() {
            let at = lba + (done / 512) as u64;
            if at >= self.blocks {
                return Err(BlockError::OutOfRange);
            }
            self.fill_cache(at & !31)?;
            let off = ((at - self.cache_lba) as usize) * 512;
            let n = (buf.len() - done).min(512);
            buf[done..done + n].copy_from_slice(&self.cache[off..off + n]);
            done += n;
        }
        Ok(())
    }

    fn write_blocks(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        if buf.len() % 512 != 0 {
            return Err(BlockError::Io);
        }
        self.cache_ok = false;
        let mut done = 0usize;
        while done < buf.len() {
            let n = (buf.len() - done).min(4096);
            let at = lba + (done / 512) as u64;
            if at >= self.blocks {
                return Err(BlockError::OutOfRange);
            }
            self.host
                .write_at(at as u32, &buf[done..done + n])
                .map_err(|_| BlockError::Io)?;
            done += n;
        }
        Ok(())
    }

    fn sync(&mut self) -> Result<(), BlockError> {
        self.host.sync_cache().map_err(|_| BlockError::Io)
    }
}

impl Disk {
    fn fill_cache(&mut self, base: u64) -> Result<(), BlockError> {
        if self.cache_ok && self.cache_lba == base {
            return Ok(());
        }
        for part in 0..4 {
            let lba = base + part * 8;
            let start = part as usize * 4096;
            self.host
                .read_at(lba as u32, &mut self.cache[start..start + 4096])
                .map_err(|_| BlockError::Io)?;
        }
        self.cache_lba = base;
        self.cache_ok = true;
        Ok(())
    }
}
