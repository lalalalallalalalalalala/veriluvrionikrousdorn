//! Драйверы устройств. Каждый подмодуль отвечает за одно устройство.
//!
//! Общий контракт для будущей файловой системы — [`block::BlockDevice`].
//! Реализации ATA/AHCI появятся вместе с Btrfs и сюда же подключатся.

#![no_std]

pub mod ata;
pub mod block;
pub mod cyr;
pub mod devices;
pub mod display;
pub mod event;
pub mod hid;
pub mod input;
pub mod keyboard;
pub mod layout;
pub mod mouse;
pub mod pci;
pub mod ramdisk;
pub mod rtc;
pub mod serial;
pub mod settings;
pub mod timer;
pub mod usb_hid;
pub mod usb_msc;
pub mod xhci;

mod font;

pub fn with_root_disk<R>(f: impl FnOnce(&mut dyn block::BlockDevice) -> R) -> Result<R, block::BlockError> {
    if usb_msc::active() {
        usb_msc::with_disk(|disk| f(disk))
    } else if ramdisk::active() {
        ramdisk::with_disk(|disk| f(disk))
    } else {
        ata::with_disk(|disk| f(disk))
    }
}
