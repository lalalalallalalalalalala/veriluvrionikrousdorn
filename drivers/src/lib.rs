//! Драйверы устройств. Каждый подмодуль отвечает за одно устройство.
//!
//! Общий контракт для будущей файловой системы — [`block::BlockDevice`].
//! Реализации ATA/AHCI появятся вместе с Btrfs и сюда же подключатся.

#![no_std]

pub mod ata;
pub mod block;
pub mod display;
pub mod input;
pub mod keyboard;
pub mod mouse;
pub mod ramdisk;
pub mod rtc;
pub mod serial;
pub mod timer;

mod font;

pub fn with_root_disk<R>(f: impl FnOnce(&mut dyn block::BlockDevice) -> R) -> Result<R, block::BlockError> {
    if ramdisk::active() {
        ramdisk::with_disk(|disk| f(disk))
    } else {
        ata::with_disk(|disk| f(disk))
    }
}
