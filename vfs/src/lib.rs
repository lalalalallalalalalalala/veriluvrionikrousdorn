//! Виртуальная файловая система.
//!
//! Слой не знает про Btrfs напрямую: файловая система реализует [`Filesystem`]
//! и сидит на [`BlockDevice`]. Таблица монтирования появится вместе с кучей.
//! Пока любая операция сообщает, что реализация ещё не написана.

#![no_std]

pub use vlr_drivers::block::{BlockDevice, BlockError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsType {
    Btrfs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfsError {
    NotImplemented,
    NotFound,
    InvalidPath,
    Io,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeId(pub u64);

#[derive(Clone, Copy, Debug)]
pub struct DirEntry {
    pub name: [u8; 255],
    pub name_len: u8,
    pub node: NodeId,
}

#[derive(Clone, Copy, Debug)]
pub struct MountOptions {
    /// Первая рабочая версия Btrfs будет только для чтения.
    pub read_only: bool,
}

pub trait Filesystem {
    fn fs_type(&self) -> FsType;
    fn lookup(&mut self, path: &str) -> Result<NodeId, VfsError>;
    fn read(&mut self, node: NodeId, offset: u64, buf: &mut [u8]) -> Result<usize, VfsError>;
    fn readdir(&mut self, node: NodeId, index: usize) -> Result<Option<DirEntry>, VfsError>;
}

pub struct Vfs;

impl Vfs {
    pub const fn new() -> Self {
        Self
    }

    pub fn mount(
        &mut self,
        _path: &str,
        _fs: FsType,
        _options: MountOptions,
    ) -> Result<(), VfsError> {
        Err(VfsError::NotImplemented)
    }

    pub fn open(&mut self, _path: &str) -> Result<NodeId, VfsError> {
        Err(VfsError::NotImplemented)
    }
}

pub fn init() -> Vfs {
    Vfs::new()
}
