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
pub mod rtc;
pub mod serial;
pub mod timer;

mod font;
