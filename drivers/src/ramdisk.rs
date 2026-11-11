//! Корень в оперативной памяти. Живой USB кладёт сюда образ Btrfs,
//! который загрузчик уже прочитал с носителя. Запись не возвращается на диск.

use spin::Mutex;

use super::block::{BlockDevice, BlockError};

static RAM: Mutex<Option<RamDisk>> = Mutex::new(None);

pub struct RamDisk {
    ptr: *mut u8,
    len: usize,
}

unsafe impl Send for RamDisk {}

pub fn init(ptr: *mut u8, len: usize) {
    if ptr.is_null() || len < 512 {
        return;
    }
    *RAM.lock() = Some(RamDisk { ptr, len });
}

pub fn active() -> bool {
    RAM.lock().is_some()
}

pub fn with_disk<R>(f: impl FnOnce(&mut RamDisk) -> R) -> Result<R, BlockError> {
    let mut guard = RAM.lock();
    let disk = guard.as_mut().ok_or(BlockError::Io)?;
    Ok(f(disk))
}

impl BlockDevice for RamDisk {
    fn block_size(&self) -> u64 {
        512
    }
    fn block_count(&self) -> u64 {
        (self.len / 512) as u64
    }
    fn read_blocks(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        self.copy(lba, buf, true)
    }
    fn write_blocks(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        self.copy_from(lba, buf)
    }
}

impl RamDisk {
    fn copy(&mut self, lba: u64, buf: &mut [u8], _read: bool) -> Result<(), BlockError> {
        let start = lba as usize * 512;
        if start.saturating_add(buf.len()) > self.len {
            return Err(BlockError::OutOfRange);
        }
        unsafe {
            core::ptr::copy_nonoverlapping(self.ptr.add(start), buf.as_mut_ptr(), buf.len());
        }
        Ok(())
    }

    fn copy_from(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        let start = lba as usize * 512;
        if start.saturating_add(buf.len()) > self.len {
            return Err(BlockError::OutOfRange);
        }
        unsafe {
            core::ptr::copy_nonoverlapping(buf.as_ptr(), self.ptr.add(start), buf.len());
        }
        Ok(())
    }
}
