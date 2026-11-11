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
    ReadOnly,
    Exists,
    NotEmpty,
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

pub fn create_file(path: &str) -> Result<(), VfsError> {
    vlr_btrfs::create_file(path).map_err(map_err)
}

pub fn make_dir(path: &str) -> Result<(), VfsError> {
    vlr_btrfs::make_dir(path).map_err(map_err)
}

pub fn remove_path(path: &str) -> Result<(), VfsError> {
    vlr_btrfs::remove_path(path).map_err(map_err)
}

pub fn rename_path(from: &str, to: &str) -> Result<(), VfsError> {
    vlr_btrfs::rename_path(from, to).map_err(map_err)
}

pub fn write_file(path: &str, data: &[u8]) -> Result<(), VfsError> {
    if !vlr_btrfs::may_write(path) {
        return Err(VfsError::ReadOnly);
    }
    let created = vlr_btrfs::lookup_inode(path).is_err();
    if created {
        vlr_btrfs::create_file(path).map_err(map_err)?;
    }
    let inode = vlr_btrfs::lookup_inode(path).map_err(map_err)?;
    vlr_btrfs::write_inode(inode, 0, data, false, true).map_err(map_err)
}

pub fn write_at(inode: u64, offset: u64, data: &[u8], append: bool) -> Result<(), VfsError> {
    vlr_btrfs::write_inode(inode, offset, data, append, false).map_err(map_err)
}

fn map_err(err: vlr_btrfs::BtrfsError) -> VfsError {
    match err {
        vlr_btrfs::BtrfsError::NotFound => VfsError::NotFound,
        vlr_btrfs::BtrfsError::NotDirectory => VfsError::NotDirectory,
        vlr_btrfs::BtrfsError::Compressed => VfsError::Compressed,
        vlr_btrfs::BtrfsError::Unsupported => VfsError::Unsupported,
        vlr_btrfs::BtrfsError::ReadOnly => VfsError::ReadOnly,
        vlr_btrfs::BtrfsError::Exists => VfsError::Exists,
        vlr_btrfs::BtrfsError::NotEmpty => VfsError::NotEmpty,
        vlr_btrfs::BtrfsError::NotFile | vlr_btrfs::BtrfsError::InvalidPath => VfsError::InvalidPath,
        vlr_btrfs::BtrfsError::BadMagic | vlr_btrfs::BtrfsError::Io => VfsError::Io,
    }
}
