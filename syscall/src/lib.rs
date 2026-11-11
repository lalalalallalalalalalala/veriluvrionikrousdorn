//! Диспетчер `int 0x80`. Номера те же, что в `vlr-libc`.
//!
//! Указатели из кольца 3 принимаются только если каждый байт лежит в окне
//! пользователя и страница помечена USER. Иначе вызов возвращает `-1`, а ядро
//! не читает чужую память.

#![no_std]

extern crate alloc;

use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};

use vlr_arch::syscall_entry::SyscallFrame;
use vlr_libc::{
    FD_STDIN, FD_STDOUT, SYS_CLOSE, SYS_EXEC, SYS_EXIT, SYS_GETPID, SYS_GETTICKS, SYS_KILL,
    SYS_FSYNC, SYS_KSPAWN, SYS_MEMSTAT, SYS_MKDIR, SYS_OPEN, SYS_READ, SYS_READDIR, SYS_RENAME,
    SYS_SLEEP, SYS_TASKSTAT, SYS_UNLINK, SYS_WAIT, SYS_WRITE, SYS_YIELD, O_APPEND, O_CREAT, O_RDWR,
    O_TRUNC, O_WRONLY, TaskRec, TwoPaths,
};

static DEMO: AtomicU64 = AtomicU64::new(0);

pub fn set_demo_spawner(spawn: fn(&str) -> Result<u64, &'static str>) {
    DEMO.store(spawn as usize as u64, Ordering::Relaxed);
}

const FAULT: u64 = (-1i64) as u64;
const EROFS: u64 = (-3i64) as u64;
const EEXIST: u64 = (-4i64) as u64;
const ENOTEMPTY: u64 = (-5i64) as u64;

pub fn init() {
    vlr_arch::set_syscall_handler(handle);
}

unsafe extern "C" fn handle(frame: *mut SyscallFrame) {
    if frame.is_null() {
        return;
    }
    let frame = unsafe { &mut *frame };
    let result = dispatch(frame.rax, frame.rdi, frame.rsi, frame.rdx);
    frame.rax = result;
}

fn dispatch(number: u64, a: u64, b: u64, c: u64) -> u64 {
    match number {
        SYS_READ => sys_read(a, b, c),
        SYS_WRITE => sys_write(a, b, c),
        SYS_EXIT => {
            let _ = a;
            vlr_sched::exit();
        }
        SYS_YIELD => {
            vlr_sched::yield_now();
            0
        }
        SYS_SLEEP => {
            vlr_sched::sleep(a);
            0
        }
        SYS_GETPID => vlr_sched::current_id(),
        SYS_GETTICKS => vlr_drivers::timer::ticks(),
        SYS_OPEN => sys_open(a, b, c),
        SYS_CLOSE => {
            if vlr_sched::fd_close(a) {
                0
            } else {
                FAULT
            }
        }
        SYS_READDIR => sys_readdir(a, b, c),
        SYS_EXEC => sys_exec(a, b),
        SYS_WAIT => {
            while vlr_sched::alive(a) {
                vlr_sched::sleep(1);
            }
            0
        }
        SYS_MEMSTAT => sys_memstat(a),
        SYS_TASKSTAT => sys_taskstat(a, b),
        SYS_KILL => match vlr_sched::kill(a) {
            Ok(()) => 0,
            Err(_) => FAULT,
        },
        SYS_KSPAWN => sys_kspawn(a, b),
        SYS_MKDIR => sys_path(a, b, vlr_btrfs::make_dir),
        SYS_UNLINK => sys_path(a, b, vlr_btrfs::remove_path),
        SYS_RENAME => sys_rename(a),
        SYS_FSYNC => 0,
        _ => FAULT,
    }
}

fn sys_write(fd: u64, ptr: u64, len: u64) -> u64 {
    if fd != FD_STDOUT {
        return sys_write_file(fd, ptr, len);
    }
    let len = len.min(4096) as usize;
    if !vlr_memory::is_user_range(ptr, len) {
        return FAULT;
    }
    let mut buf = [0u8; 256];
    let mut done = 0usize;
    while done < len {
        let chunk = (len - done).min(buf.len());
        unsafe {
            core::ptr::copy_nonoverlapping(
                (ptr + done as u64) as *const u8,
                buf.as_mut_ptr(),
                chunk,
            );
        }
        if &buf[..chunk] == b"\x1b[2J\x1b[H" && chunk == len {
            vlr_console::clear();
        } else if buf[..chunk].contains(&0x08) {
            for &byte in &buf[..chunk] {
                if byte == 0x08 || byte == 0x7F {
                    vlr_console::backspace();
                } else if let Ok(text) = core::str::from_utf8(&[byte]) {
                    vlr_console::write_str(text);
                }
            }
        } else {
            let text = core::str::from_utf8(&buf[..chunk]).unwrap_or("?");
            vlr_console::write_str(text);
        }
        done += chunk;
    }
    len as u64
}

fn sys_read(fd: u64, ptr: u64, len: u64) -> u64 {
    if fd != FD_STDIN {
        return sys_read_file(fd, ptr, len);
    }
    let len = len.min(4096) as usize;
    if len == 0 {
        return 0;
    }
    if !vlr_memory::is_user_range(ptr, len) {
        return FAULT;
    }
    let mut done = 0usize;
    while done < len {
        if let Some(byte) = vlr_drivers::input::pop() {
            unsafe {
                ((ptr + done as u64) as *mut u8).write(byte);
            }
            done += 1;
            if byte == b'\n' {
                break;
            }
        } else if done > 0 {
            break;
        } else {
            vlr_sched::sleep(1);
        }
    }
    done as u64
}

fn sys_read_file(fd: u64, ptr: u64, len: u64) -> u64 {
    let Some((inode, pos)) = vlr_sched::fd_read(fd) else {
        return FAULT;
    };
    let len = len.min(4096) as usize;
    if len == 0 {
        return 0;
    }
    if !vlr_memory::is_user_range(ptr, len) {
        return FAULT;
    }
    let mut buf = [0u8; 4096];
    let n = match vlr_btrfs::read_at(inode, pos, &mut buf[..len]) {
        Ok(n) => n,
        Err(_) => return FAULT,
    };
    unsafe {
        core::ptr::copy_nonoverlapping(buf.as_ptr(), ptr as *mut u8, n);
    }
    vlr_sched::fd_advance(fd, pos + n as u64);
    n as u64
}

fn sys_open(ptr: u64, len: u64, flags: u64) -> u64 {
    let Some(path) = user_string(ptr, len) else {
        return FAULT;
    };
    let writable = flags & (O_WRONLY | O_RDWR | O_CREAT | O_TRUNC | O_APPEND) != 0;
    if writable && !vlr_btrfs::may_write(&path) {
        return EROFS;
    }
    let mut inode = vlr_btrfs::lookup_inode(&path);
    if inode.is_err() && flags & O_CREAT != 0 {
        if let Err(err) = vlr_btrfs::create_file(&path) {
            return btrfs_code(err);
        }
        inode = vlr_btrfs::lookup_inode(&path);
    }
    let Ok(inode) = inode else {
        return FAULT;
    };
    if flags & O_TRUNC != 0 {
        if let Err(err) = vlr_btrfs::write_inode(inode, 0, &[], false, true) {
            return btrfs_code(err);
        }
    }
    vlr_sched::fd_open(inode, writable, flags & O_APPEND != 0).unwrap_or(FAULT)
}

fn sys_write_file(fd: u64, ptr: u64, len: u64) -> u64 {
    let Some(meta) = vlr_sched::fd_meta(fd) else {
        return FAULT;
    };
    if !meta.writable {
        return EROFS;
    }
    let len = len.min(4096) as usize;
    if len == 0 {
        return 0;
    }
    if !vlr_memory::is_user_range(ptr, len) {
        return FAULT;
    }
    let mut buf = alloc::vec![0u8; len];
    unsafe {
        core::ptr::copy_nonoverlapping(ptr as *const u8, buf.as_mut_ptr(), len);
    }
    if vlr_btrfs::write_inode(meta.inode, meta.pos, &buf, meta.append, false).is_err() {
        return FAULT;
    }
    if !meta.append {
        vlr_sched::fd_advance(fd, meta.pos + len as u64);
    }
    len as u64
}

fn sys_path(ptr: u64, len: u64, op: fn(&str) -> Result<(), vlr_btrfs::BtrfsError>) -> u64 {
    let Some(path) = user_string(ptr, len) else {
        return FAULT;
    };
    match op(&path) {
        Ok(()) => 0,
        Err(err) => btrfs_code(err),
    }
}

fn sys_rename(ptr: u64) -> u64 {
    let size = core::mem::size_of::<TwoPaths>();
    if !vlr_memory::is_user_range(ptr, size) {
        return FAULT;
    }
    let args = unsafe { core::ptr::read(ptr as *const TwoPaths) };
    let Some(from) = user_string(args.first, args.first_len) else {
        return FAULT;
    };
    let Some(to) = user_string(args.second, args.second_len) else {
        return FAULT;
    };
    match vlr_btrfs::rename_path(&from, &to) {
        Ok(()) => 0,
        Err(err) => btrfs_code(err),
    }
}

fn btrfs_code(err: vlr_btrfs::BtrfsError) -> u64 {
    match err {
        vlr_btrfs::BtrfsError::ReadOnly => EROFS,
        vlr_btrfs::BtrfsError::Exists => EEXIST,
        vlr_btrfs::BtrfsError::NotEmpty => ENOTEMPTY,
        _ => FAULT,
    }
}

fn sys_readdir(fd: u64, index: u64, ptr: u64) -> u64 {
    let Some((inode, _)) = vlr_sched::fd_read(fd) else {
        return FAULT;
    };
    if !vlr_memory::is_user_range(ptr, 64) {
        return FAULT;
    }
    let names = match vlr_btrfs::list_inode(inode) {
        Ok(names) => names,
        Err(_) => return FAULT,
    };
    let Some(name) = names.get(index as usize) else {
        return 0;
    };
    let bytes = name.as_bytes();
    let n = bytes.len().min(63);
    unsafe {
        core::ptr::write_bytes(ptr as *mut u8, 0, 64);
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, n);
    }
    n as u64
}

fn sys_exec(ptr: u64, len: u64) -> u64 {
    let Some(path) = user_string(ptr, len) else {
        return FAULT;
    };
    match vlr_loader::exec_path(&path) {
        Ok(pid) => pid,
        Err(vlr_loader::LoadError::BadImage | vlr_loader::LoadError::Unsupported) => {
            (-2i64) as u64
        }
        Err(_) => FAULT,
    }
}

fn sys_memstat(ptr: u64) -> u64 {
    let size = core::mem::size_of::<vlr_libc::MemInfo>();
    if !vlr_memory::is_user_range(ptr, size) {
        return FAULT;
    }
    let stats = vlr_memory::stats();
    let info = vlr_libc::MemInfo {
        frames_total: stats.frames_total as u64,
        frames_used: stats.frames_used as u64,
        frames_free: stats.frames_free as u64,
        heap_total: stats.heap_total as u64,
        heap_used: stats.heap_used as u64,
        heap_free: stats.heap_free as u64,
    };
    unsafe {
        core::ptr::write(ptr as *mut vlr_libc::MemInfo, info);
    }
    0
}

fn sys_taskstat(ptr: u64, count: u64) -> u64 {
    let count = count.min(32) as usize;
    let size = count * core::mem::size_of::<TaskRec>();
    if count == 0 || !vlr_memory::is_user_range(ptr, size) {
        return FAULT;
    }
    let mut infos = [vlr_sched::TaskInfo::empty(); 32];
    let n = vlr_sched::snapshot(&mut infos).min(count);
    for (index, info) in infos.iter().take(n).enumerate() {
        let mut rec = TaskRec {
            id: info.id,
            ticks: info.ticks,
            user: if info.mode == "user" { 1 } else { 0 },
            name: [0; 16],
            state: [0; 16],
        };
        let name_len = info.name_len as usize;
        rec.name[..name_len].copy_from_slice(&info.name[..name_len]);
        let state = info.state.as_bytes();
        let state_len = state.len().min(15);
        rec.state[..state_len].copy_from_slice(&state[..state_len]);
        unsafe {
            core::ptr::write(
                (ptr as *mut TaskRec).add(index),
                rec,
            );
        }
    }
    n as u64
}

fn sys_kspawn(ptr: u64, len: u64) -> u64 {
    let Some(name) = user_string(ptr, len) else {
        return FAULT;
    };
    let spawn = DEMO.load(Ordering::Relaxed);
    if spawn == 0 {
        return FAULT;
    }
    let spawn: fn(&str) -> Result<u64, &'static str> = unsafe { core::mem::transmute(spawn as usize) };
    match spawn(&name) {
        Ok(id) => id,
        Err(_) => FAULT,
    }
}

fn user_string(ptr: u64, len: u64) -> Option<String> {
    let len = len.min(255) as usize;
    if len == 0 || !vlr_memory::is_user_range(ptr, len) {
        return None;
    }
    let mut buf = [0u8; 256];
    unsafe {
        core::ptr::copy_nonoverlapping(ptr as *const u8, buf.as_mut_ptr(), len);
    }
    core::str::from_utf8(&buf[..len]).ok().map(String::from)
}
