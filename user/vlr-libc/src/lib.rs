//! Тонкая библиотека системных вызовов для будущих программ `.vlr`, включая `vch`.
//!
//! Номера совпадают с `vlr-syscall`. Вход — `int 0x80`: `rax` номер, `rdi`/`rsi`/`rdx`
//! аргументы, результат в `rax`. Отрицательное значение — ошибка (`-1` чаще всего
//! значит плохой указатель или дескриптор).

#![no_std]

pub const SYS_READ: u64 = 1;
pub const SYS_WRITE: u64 = 2;
pub const SYS_EXIT: u64 = 3;
pub const SYS_YIELD: u64 = 4;
pub const SYS_SLEEP: u64 = 5;
pub const SYS_GETPID: u64 = 6;
pub const SYS_GETTICKS: u64 = 7;

pub const FD_STDIN: u64 = 0;
pub const FD_STDOUT: u64 = 1;

pub fn syscall0(number: u64) -> u64 {
    syscall3(number, 0, 0, 0)
}

pub fn syscall1(number: u64, a: u64) -> u64 {
    syscall3(number, a, 0, 0)
}

pub fn syscall3(number: u64, a: u64, b: u64, c: u64) -> u64 {
    let result: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inout("rax") number => result,
            in("rdi") a,
            in("rsi") b,
            in("rdx") c,
            out("rcx") _,
            out("r11") _,
        );
    }
    result
}

pub fn read(fd: u64, buf: &mut [u8]) -> i64 {
    syscall3(SYS_READ, fd, buf.as_mut_ptr() as u64, buf.len() as u64) as i64
}

pub fn write(fd: u64, buf: &[u8]) -> i64 {
    syscall3(SYS_WRITE, fd, buf.as_ptr() as u64, buf.len() as u64) as i64
}

pub fn exit(code: u64) -> ! {
    syscall1(SYS_EXIT, code);
    loop {}
}

pub fn yield_now() {
    let _ = syscall0(SYS_YIELD);
}

pub fn sleep(ticks: u64) {
    let _ = syscall1(SYS_SLEEP, ticks);
}

pub fn getpid() -> u64 {
    syscall0(SYS_GETPID)
}

pub fn getticks() -> u64 {
    syscall0(SYS_GETTICKS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_stable() {
        let nums = [
            SYS_READ,
            SYS_WRITE,
            SYS_EXIT,
            SYS_YIELD,
            SYS_SLEEP,
            SYS_GETPID,
            SYS_GETTICKS,
        ];
        for (index, number) in nums.iter().enumerate() {
            assert_eq!(*number, (index as u64) + 1);
        }
    }
}
