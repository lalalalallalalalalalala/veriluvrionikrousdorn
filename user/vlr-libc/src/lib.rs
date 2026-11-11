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
pub const SYS_READDIR: u64 = 8;
pub const SYS_OPEN: u64 = 9;
pub const SYS_CLOSE: u64 = 10;
pub const SYS_EXEC: u64 = 11;
pub const SYS_WAIT: u64 = 12;
pub const SYS_MEMSTAT: u64 = 13;
pub const SYS_TASKSTAT: u64 = 14;
pub const SYS_KILL: u64 = 15;
pub const SYS_KSPAWN: u64 = 16;

pub const FD_STDIN: u64 = 0;
pub const FD_STDOUT: u64 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MemInfo {
    pub frames_total: u64,
    pub frames_used: u64,
    pub frames_free: u64,
    pub heap_total: u64,
    pub heap_used: u64,
    pub heap_free: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TaskRec {
    pub id: u64,
    pub ticks: u64,
    pub user: u64,
    pub name: [u8; 16],
    pub state: [u8; 16],
}

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

pub fn open(path: &str) -> i64 {
    syscall3(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, 0) as i64
}

pub fn close(fd: u64) -> i64 {
    syscall1(SYS_CLOSE, fd) as i64
}

pub fn readdir(fd: u64, index: u64, buf: &mut [u8]) -> i64 {
    syscall3(SYS_READDIR, fd, index, buf.as_mut_ptr() as u64) as i64
}

pub fn exec(path: &str) -> i64 {
    syscall3(SYS_EXEC, path.as_ptr() as u64, path.len() as u64, 0) as i64
}

pub fn wait(pid: u64) -> i64 {
    syscall1(SYS_WAIT, pid) as i64
}

pub fn memstat(info: &mut MemInfo) -> i64 {
    syscall1(SYS_MEMSTAT, info as *mut MemInfo as u64) as i64
}

pub fn taskstat(buf: &mut [TaskRec]) -> i64 {
    syscall3(SYS_TASKSTAT, buf.as_mut_ptr() as u64, buf.len() as u64, 0) as i64
}

pub fn kill(pid: u64) -> i64 {
    syscall1(SYS_KILL, pid) as i64
}

pub fn kspawn(name: &str) -> i64 {
    syscall3(SYS_KSPAWN, name.as_ptr() as u64, name.len() as u64, 0) as i64
}

#[cfg(feature = "runtime")]
mod rt {
    use core::alloc::{GlobalAlloc, Layout};
    use core::sync::atomic::{AtomicUsize, Ordering};

    const HEAP_SIZE: usize = 128 * 1024;

    struct Heap(core::cell::UnsafeCell<[u8; HEAP_SIZE]>);
    unsafe impl Sync for Heap {}

    static HEAP: Heap = Heap(core::cell::UnsafeCell::new([0; HEAP_SIZE]));
    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Bump;

    unsafe impl GlobalAlloc for Bump {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let align = layout.align().max(1);
            let mut current = NEXT.load(Ordering::Relaxed);
            loop {
                let aligned = (current + align - 1) & !(align - 1);
                let end = aligned.saturating_add(layout.size());
                if end > HEAP_SIZE {
                    return core::ptr::null_mut();
                }
                match NEXT.compare_exchange_weak(
                    current,
                    end,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return unsafe { (HEAP.0.get() as *mut u8).add(aligned) },
                    Err(actual) => current = actual,
                }
            }
        }

        unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
    }

    #[global_allocator]
    static ALLOCATOR: Bump = Bump;

    #[panic_handler]
    fn panic(_info: &core::panic::PanicInfo) -> ! {
        let _ = crate::write(crate::FD_STDOUT, b"panic\n");
        crate::exit(1);
    }
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
            SYS_READDIR,
            SYS_OPEN,
            SYS_CLOSE,
            SYS_EXEC,
            SYS_WAIT,
            SYS_MEMSTAT,
            SYS_TASKSTAT,
            SYS_KILL,
            SYS_KSPAWN,
        ];
        for (index, number) in nums.iter().enumerate() {
            assert_eq!(*number, (index as u64) + 1);
        }
    }
}
