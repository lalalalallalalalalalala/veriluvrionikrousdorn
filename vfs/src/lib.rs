//! Виртуальная файловая система поверх Btrfs только для чтения.
//!
//! Остальное ядро открывает пути здесь и не знает про чанки и деревья.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub use vlr_drivers::block::{BlockDevice, BlockError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsType {
    Btrfs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfsError {
    NotMounted,
    NotFound,
    NotDirectory,
    InvalidPath,
    Io,
    Compressed,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeId(pub u64);

#[derive(Clone, Debug)]
pub struct DirEntry {
    pub name: String,
}

pub fn mount() -> Result<(), VfsError> {
    vlr_btrfs::mount_global().map_err(map_err)
}

pub fn read_file(path: &str) -> Result<Vec<u8>, VfsError> {
    vlr_btrfs::read_path(path).map_err(map_err)
}

pub fn list_dir(path: &str) -> Result<Vec<String>, VfsError> {
    vlr_btrfs::list_path(path).map_err(map_err)
}

fn map_err(err: vlr_btrfs::BtrfsError) -> VfsError {
    match err {
        vlr_btrfs::BtrfsError::NotFound => VfsError::NotFound,
        vlr_btrfs::BtrfsError::NotDirectory => VfsError::NotDirectory,
        vlr_btrfs::BtrfsError::Compressed => VfsError::Compressed,
        vlr_btrfs::BtrfsError::Unsupported => VfsError::Unsupported,
        vlr_btrfs::BtrfsError::BadMagic | vlr_btrfs::BtrfsError::Io => VfsError::Io,
    }
}
