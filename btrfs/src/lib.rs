//! Btrfs для VLR.
//!
//! В этой вехе нет разбора суперблока и деревьев. Модуль занимает место в
//! дереве зависимостей: том реализует [`vlr_vfs::Filesystem`] и открывается
//! поверх [`vlr_drivers::block::BlockDevice`].
//!
//! Порядок работ, когда дойдёт очередь: сначала чтение (поиск, каталоги, файлы),
//! затем запись. Магическая подпись суперблока зафиксирована заранее.

#![no_std]

use vlr_drivers::block::BlockDevice;
use vlr_vfs::{DirEntry, Filesystem, FsType, NodeId, VfsError};

/// Смещение суперблока Btrfs от начала устройства.
pub const SUPERBLOCK_OFFSET: u64 = 0x1_0000;
/// ASCII `_BHRfS_M`.
pub const MAGIC: &[u8; 8] = b"_BHRfS_M";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtrfsError {
    NotImplemented,
    Io,
    BadMagic,
}

pub struct BtrfsVolume;

impl BtrfsVolume {
    /// Открыть том только для чтения. Пока всегда [`BtrfsError::NotImplemented`].
    pub fn open_read_only(_device: &mut dyn BlockDevice) -> Result<Self, BtrfsError> {
        Err(BtrfsError::NotImplemented)
    }

    /// Запись появится после устойчивого чтения.
    pub fn open_read_write(_device: &mut dyn BlockDevice) -> Result<Self, BtrfsError> {
        Err(BtrfsError::NotImplemented)
    }
}

impl Filesystem for BtrfsVolume {
    fn fs_type(&self) -> FsType {
        FsType::Btrfs
    }

    fn lookup(&mut self, _path: &str) -> Result<NodeId, VfsError> {
        Err(VfsError::NotImplemented)
    }

    fn read(&mut self, _node: NodeId, _offset: u64, _buf: &mut [u8]) -> Result<usize, VfsError> {
        Err(VfsError::NotImplemented)
    }

    fn readdir(&mut self, _node: NodeId, _index: usize) -> Result<Option<DirEntry>, VfsError> {
        Err(VfsError::NotImplemented)
    }
}

pub fn init() {}
