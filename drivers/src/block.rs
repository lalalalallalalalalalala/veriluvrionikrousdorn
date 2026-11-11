//! Контракт блочного устройства. Файловые системы (сначала Btrfs) сидят на нём,
//! а не на конкретном контроллере диска.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// Драйвер контроллера ещё не написан, либо операция не поддерживается.
    Unsupported,
    OutOfRange,
    Io,
}

/// Устройство, которое читает и пишет целые блоки (обычно 512 или 4096 байт).
///
/// Трейт объектно-безопасен, чтобы VFS мог держать том, не зная тип контроллера.
pub trait BlockDevice {
    fn block_size(&self) -> u64;
    fn block_count(&self) -> u64;
    fn read_blocks(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError>;
    fn write_blocks(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError>;

    /// Сбросить кэш устройства. По умолчанию запись уже устойчива.
    fn sync(&mut self) -> Result<(), BlockError> {
        Ok(())
    }
}
