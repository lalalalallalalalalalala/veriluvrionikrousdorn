//! ATA PIO, primary slave (`0x1F0`, drive 1).
//!
//! Загрузочный диск QEMU сидит на primary master. Образ Btrfs — второй IDE-диск,
//! то есть slave. PIO выбран потому, что эмуляция IDE в QEMU стабильна и не
//! требует разбора PCI и очередей virtio: для чтения секторов этого достаточно.

use spin::Mutex;
use x86_64::instructions::port::Port;

use super::block::{BlockDevice, BlockError};

static DISK: Mutex<Option<AtaDisk>> = Mutex::new(None);

pub fn init() -> Result<(), BlockError> {
    let disk = AtaDisk::init()?;
    *DISK.lock() = Some(disk);
    Ok(())
}

pub fn with_disk<R>(f: impl FnOnce(&mut AtaDisk) -> R) -> Result<R, BlockError> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut guard = DISK.lock();
        let disk = guard.as_mut().ok_or(BlockError::Io)?;
        Ok(f(disk))
    })
}

const DATA: u16 = 0x1F0;
const SECTOR_COUNT: u16 = 0x1F2;
const LBA_LO: u16 = 0x1F3;
const LBA_MID: u16 = 0x1F4;
const LBA_HI: u16 = 0x1F5;
const DRIVE: u16 = 0x1F6;
const STATUS: u16 = 0x1F7;
const COMMAND: u16 = 0x1F7;

const BSY: u8 = 0x80;
const DRQ: u8 = 0x08;
const ERR: u8 = 0x01;
const SECTOR: usize = 512;

pub struct AtaDisk {
    sectors: u64,
}

impl AtaDisk {
    pub fn init() -> Result<Self, BlockError> {
        // 0xF0: slave, LBA. Несколько чтений status — пауза после выбора диска.
        unsafe {
            Port::<u8>::new(DRIVE).write(0xF0);
            for _ in 0..4 {
                let _ = Port::<u8>::new(STATUS).read();
            }
        }
        wait_not_busy()?;
        // IDENTIFY
        unsafe { Port::<u8>::new(COMMAND).write(0xEC) };
        let status = unsafe { Port::<u8>::new(STATUS).read() };
        if status == 0 {
            return Err(BlockError::Io);
        }
        wait_drq()?;
        let mut id = [0u16; 256];
        unsafe {
            let mut data = Port::<u16>::new(DATA);
            for word in &mut id {
                *word = data.read();
            }
        }
        let sectors = (id[61] as u64) << 16 | id[60] as u64;
        if sectors == 0 {
            return Err(BlockError::Io);
        }
        Ok(Self { sectors })
    }

    pub fn read_bytes(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let mut done = 0usize;
        while done < buf.len() {
            let pos = offset + done as u64;
            let lba = pos / SECTOR as u64;
            let skip = (pos % SECTOR as u64) as usize;
            let mut sector = [0u8; SECTOR];
            self.read_sector(lba, &mut sector)?;
            let n = (buf.len() - done).min(SECTOR - skip);
            buf[done..done + n].copy_from_slice(&sector[skip..skip + n]);
            done += n;
        }
        Ok(())
    }

    fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if lba >= self.sectors || buf.len() < SECTOR {
            return Err(BlockError::OutOfRange);
        }
        wait_not_busy()?;
        unsafe {
            let head = 0xF0 | ((lba >> 24) as u8 & 0x0F);
            Port::<u8>::new(DRIVE).write(head);
            Port::<u8>::new(SECTOR_COUNT).write(1);
            Port::<u8>::new(LBA_LO).write(lba as u8);
            Port::<u8>::new(LBA_MID).write((lba >> 8) as u8);
            Port::<u8>::new(LBA_HI).write((lba >> 16) as u8);
            Port::<u8>::new(COMMAND).write(0x20);
        }
        wait_drq()?;
        unsafe {
            let mut data = Port::<u16>::new(DATA);
            for i in 0..256 {
                let word = data.read();
                buf[i * 2] = word as u8;
                buf[i * 2 + 1] = (word >> 8) as u8;
            }
        }
        Ok(())
    }
}

impl BlockDevice for AtaDisk {
    fn block_size(&self) -> u64 {
        SECTOR as u64
    }
    fn block_count(&self) -> u64 {
        self.sectors
    }
    fn read_blocks(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if buf.len() % SECTOR != 0 {
            return Err(BlockError::Io);
        }
        let mut offset = 0;
        let mut sector = lba;
        while offset < buf.len() {
            self.read_sector(sector, &mut buf[offset..offset + SECTOR])?;
            offset += SECTOR;
            sector += 1;
        }
        Ok(())
    }
    fn write_blocks(&mut self, _lba: u64, _buf: &[u8]) -> Result<(), BlockError> {
        Err(BlockError::Unsupported)
    }
}

fn status() -> u8 {
    unsafe { Port::<u8>::new(STATUS).read() }
}

fn wait_not_busy() -> Result<(), BlockError> {
    for _ in 0..1_000_000 {
        let s = status();
        if s & BSY == 0 {
            return Ok(());
        }
    }
    Err(BlockError::Io)
}

fn wait_drq() -> Result<(), BlockError> {
    for _ in 0..1_000_000 {
        let s = status();
        if s & ERR != 0 {
            return Err(BlockError::Io);
        }
        if s & BSY == 0 && s & DRQ != 0 {
            return Ok(());
        }
    }
    Err(BlockError::Io)
}
