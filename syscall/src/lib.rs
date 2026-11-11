//! Системные вызовы. Номера зарезервированы, диспетчер пока отклоняет всё.
//!
//! Когда появится переход из кольца 3, обработчик ловушки в `vlr-arch` будет
//! звать [`dispatch`], а не разбирать номера сам.

#![no_std]

pub const SYS_READ: usize = 1;
pub const SYS_WRITE: usize = 2;
pub const SYS_EXIT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallError {
    NotImplemented,
    UnknownNumber,
}

pub fn init() {}

pub fn dispatch(number: usize, _args: &[u64]) -> Result<u64, SyscallError> {
    match number {
        SYS_READ | SYS_WRITE | SYS_EXIT => Err(SyscallError::NotImplemented),
        _ => Err(SyscallError::UnknownNumber),
    }
}
