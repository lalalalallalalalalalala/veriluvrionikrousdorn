#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use vch::{MemorySnapshot, ShellInfo, ShellIo, TaskSnapshot};
use vlr_libc::{
    close, exec, fsync, getticks, getuser, kill, kspawn, memstat, mkdir, open, open_flags, read,
    readdir, rename, taskstat, unlink, write, MemInfo, TaskRec, FD_STDOUT, O_CREAT, O_RDWR,
    O_TRUNC,
};

struct SysIo {
    cwd: String,
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut raw = [0u8; 24];
    let n = getuser(&mut raw).max(0) as usize;
    let name = core::str::from_utf8(&raw[..n.min(raw.len())]).unwrap_or("user");
    let name: &'static str = alloc::boxed::Box::leak(String::from(name).into_boxed_str());
    let home = alloc::format!("/Users/{name}");
    let home: &'static str = alloc::boxed::Box::leak(home.into_boxed_str());
    let info = ShellInfo {
        username: name,
        os_name: "Veriluvrionikrousdorn",
        os_version: "1.1.0",
        usable_memory: 0,
        home_display: "~",
        home_path: home,
    };
    let mut io = SysIo {
        cwd: String::from(home),
    };
    vch::run(&mut io, &info);
}

impl ShellIo for SysIo {
    fn write_str(&mut self, text: &str) {
        let _ = write(FD_STDOUT, text.as_bytes());
    }

    fn clear(&mut self) {
        let _ = write(FD_STDOUT, b"\x1b[2J\x1b[H");
    }

    fn backspace(&mut self) {
        let _ = write(FD_STDOUT, b"\x08");
    }

    fn read_byte(&mut self) -> u8 {
        let mut buf = [0u8; 1];
        loop {
            let n = read(0, &mut buf);
            if n > 0 {
                return buf[0];
            }
            vlr_libc::yield_now();
        }
    }

    fn ticks(&mut self) -> u64 {
        getticks()
    }

    fn memory(&mut self) -> MemorySnapshot {
        let mut info = MemInfo {
            frames_total: 0,
            frames_used: 0,
            frames_free: 0,
            heap_total: 0,
            heap_used: 0,
            heap_free: 0,
        };
        let _ = memstat(&mut info);
        MemorySnapshot {
            frames_total: info.frames_total,
            frames_used: info.frames_used,
            frames_free: info.frames_free,
            heap_total: info.heap_total,
            heap_used: info.heap_used,
            heap_free: info.heap_free,
        }
    }

    fn tasks(&mut self) -> Vec<TaskSnapshot> {
        let mut recs = [TaskRec {
            id: 0,
            ticks: 0,
            user: 0,
            name: [0; 16],
            state: [0; 16],
        }; 32];
        let count = taskstat(&mut recs).max(0) as usize;
        recs[..count]
            .iter()
            .map(|rec| TaskSnapshot {
                id: rec.id,
                name: cstr(&rec.name),
                mode: if rec.user == 1 { "user" } else { "kernel" },
                state: state_name(&rec.state),
                ticks: rec.ticks,
            })
            .collect()
    }

    fn spawn_task(&mut self, name: &str) -> Result<u64, &'static str> {
        let id = kspawn(name);
        if id < 0 {
            Err("unknown task")
        } else {
            Ok(id as u64)
        }
    }

    fn kill_task(&mut self, id: u64) -> Result<(), &'static str> {
        if kill(id) < 0 {
            Err("kill failed")
        } else {
            Ok(())
        }
    }

    fn run_user(&mut self, name: &str) -> Result<u64, &'static str> {
        self.exec(&alloc::format!("/Apps/{name}/{name}.vlr"))
    }

    fn current_dir(&mut self) -> String {
        self.cwd.clone()
    }

    fn change_dir(&mut self, path: &str) {
        self.cwd = String::from(path);
    }

    fn list_dir(&mut self, path: &str) -> Result<Vec<String>, &'static str> {
        let fd = open(path);
        if fd < 0 {
            return Err("not found");
        }
        let mut names = Vec::new();
        let mut index = 0u64;
        loop {
            let mut buf = [0u8; 64];
            let n = readdir(fd as u64, index, &mut buf);
            if n <= 0 {
                break;
            }
            names.push(String::from(core::str::from_utf8(&buf[..n as usize]).unwrap_or("?")));
            index += 1;
            if index > 128 {
                break;
            }
        }
        let _ = close(fd as u64);
        Ok(names)
    }

    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, &'static str> {
        let fd = open(path);
        if fd < 0 {
            return Err("not found");
        }
        let mut out = Vec::new();
        loop {
            let mut buf = [0u8; 256];
            let n = read(fd as u64, &mut buf);
            if n < 0 {
                let _ = close(fd as u64);
                return Err("io error");
            }
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n as usize]);
            if out.len() > 64 * 1024 {
                break;
            }
        }
        let _ = close(fd as u64);
        Ok(out)
    }

    fn exec(&mut self, path: &str) -> Result<u64, &'static str> {
        match exec(path) {
            -2 => Err("rejected"),
            id if id < 0 => Err("not found"),
            id => Ok(id as u64),
        }
    }

    fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), &'static str> {
        let fd = open_flags(path, O_CREAT | O_TRUNC | O_RDWR);
        if fd < 0 {
            return Err(code_err(fd));
        }
        let mut done = 0usize;
        while done < data.len() {
            let n = write(fd as u64, &data[done..]);
            if n < 0 {
                let _ = close(fd as u64);
                return Err("failed");
            }
            if n == 0 {
                break;
            }
            done += n as usize;
        }
        let _ = fsync(fd as u64);
        let _ = close(fd as u64);
        Ok(())
    }

    fn create_file(&mut self, path: &str) -> Result<(), &'static str> {
        let fd = open_flags(path, O_CREAT | O_RDWR);
        if fd < 0 {
            return Err(code_err(fd));
        }
        let _ = close(fd as u64);
        Ok(())
    }

    fn make_dir(&mut self, path: &str) -> Result<(), &'static str> {
        code_result(mkdir(path))
    }

    fn remove_path(&mut self, path: &str) -> Result<(), &'static str> {
        code_result(unlink(path))
    }

    fn rename_path(&mut self, from: &str, to: &str) -> Result<(), &'static str> {
        code_result(rename(from, to))
    }

    fn install_app(&mut self, path: &str) -> Result<(), &'static str> {
        match vlr_libc::install(path) {
            0 => Ok(()),
            -2 => Err("rejected"),
            _ => Err("failed"),
        }
    }

    fn uninstall_app(&mut self, name: &str) -> Result<(), &'static str> {
        match vlr_libc::uninstall(name) {
            0 => Ok(()),
            _ => Err("failed"),
        }
    }
}

fn code_result(code: i64) -> Result<(), &'static str> {
    if code < 0 { Err(code_err(code)) } else { Ok(()) }
}

fn code_err(code: i64) -> &'static str {
    match code {
        -3 => "read only",
        -4 => "exists",
        -5 => "not empty",
        _ => "failed",
    }
}

fn cstr(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    String::from(core::str::from_utf8(&bytes[..end]).unwrap_or("?"))
}

fn state_name(bytes: &[u8]) -> &'static str {
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    match core::str::from_utf8(&bytes[..end]).unwrap_or("") {
        "running" => "running",
        "ready" => "ready",
        "sleeping" => "sleeping",
        "dead" => "dead",
        _ => "?",
    }
}
