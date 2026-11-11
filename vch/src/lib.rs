//! Встроенная оболочка vch.
//!
//! Это временный дом программы. Позже тот же цикл «приглашение, строка, команда»
//! переедет в `/Apps/vch/vch.vlr` и будет говорить с ядром через системные вызовы.
//! Сейчас ввод и вывод приходят через [`ShellIo`], чтобы модуль не зависел от
//! драйверов и его можно было проверить на хосте.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

const HISTORY_LIMIT: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemorySnapshot {
    pub frames_total: u64,
    pub frames_used: u64,
    pub frames_free: u64,
    pub heap_total: u64,
    pub heap_used: u64,
    pub heap_free: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskSnapshot {
    pub id: u64,
    pub name: String,
    pub state: &'static str,
    pub ticks: u64,
}

pub trait ShellIo {
    fn write_str(&mut self, text: &str);
    fn clear(&mut self);
    fn backspace(&mut self);
    fn read_byte(&mut self) -> u8;
    fn ticks(&mut self) -> u64;
    fn memory(&mut self) -> MemorySnapshot;
    fn tasks(&mut self) -> Vec<TaskSnapshot>;
    fn spawn_task(&mut self, name: &str) -> Result<u64, &'static str>;
    fn kill_task(&mut self, id: u64) -> Result<(), &'static str>;
}

pub struct ShellInfo {
    pub username: &'static str,
    pub os_name: &'static str,
    pub os_version: &'static str,
    pub usable_memory: u64,
    pub home_display: &'static str,
    pub home_path: &'static str,
}

pub fn run<I: ShellIo>(io: &mut I, info: &ShellInfo) -> ! {
    let mut history = Vec::new();
    loop {
        write_prompt(io, info.username, info.home_display);
        let line = read_line(io);
        if line.is_empty() {
            continue;
        }
        if history.len() == HISTORY_LIMIT {
            history.remove(0);
        }
        history.push(line.clone());
        dispatch(io, info, &line, &history);
    }
}

pub fn write_prompt<I: ShellIo>(io: &mut I, username: &str, cwd: &str) {
    let mut buf = [0u8; 96];
    let n = format_prompt(username, cwd, &mut buf);
    let text = core::str::from_utf8(&buf[..n]).unwrap_or("user@::vch#~ ");
    io.write_str(text);
}

/// `{username}@::vch#{cwd} `, для домашнего каталога `cwd` равен `~`.
pub fn format_prompt(username: &str, cwd: &str, out: &mut [u8]) -> usize {
    struct Buf<'a> {
        bytes: &'a mut [u8],
        used: usize,
    }
    impl Write for Buf<'_> {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            let bytes = text.as_bytes();
            let room = self.bytes.len().saturating_sub(self.used);
            let count = bytes.len().min(room);
            self.bytes[self.used..self.used + count].copy_from_slice(&bytes[..count]);
            self.used += count;
            Ok(())
        }
    }
    let mut buf = Buf {
        bytes: out,
        used: 0,
    };
    let _ = write!(buf, "{username}@::vch#{cwd} ");
    buf.used
}

pub fn read_line<I: ShellIo>(io: &mut I) -> String {
    let mut line = String::new();
    loop {
        match io.read_byte() {
            b'\n' | b'\r' => {
                io.write_str("\n");
                return line;
            }
            0x08 | 0x7F => {
                if line.pop().is_some() {
                    io.backspace();
                }
            }
            byte if (0x20..0x7F).contains(&byte) => {
                if line.len() < 1024 {
                    line.push(byte as char);
                    let text = [byte];
                    io.write_str(core::str::from_utf8(&text).unwrap_or(""));
                }
            }
            _ => {}
        }
    }
}

pub fn dispatch<I: ShellIo>(io: &mut I, info: &ShellInfo, line: &str, history: &[String]) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let mut parts = line.splitn(2, char::is_whitespace);
    let command = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    match command {
        "help" => {
            io.write_str(
                "commands:\n  \
                 help     this list\n  \
                 clear    clear the screen\n  \
                 echo     print the rest of the line\n  \
                 version  kernel version\n  \
                 about    system summary\n  \
                 pwd      current directory\n  \
                 whoami   current user\n  \
                 uname    system name\n  \
                 ticks    hardware timer ticks\n  \
                 mem      frame and heap usage\n  \
                 history  recent commands\n  \
                 ps       list tasks\n  \
                 spawn    spawn ticker or spinner\n  \
                 kill     stop a task by id\n",
            );
        }
        "clear" => io.clear(),
        "echo" => {
            io.write_str(rest);
            io.write_str("\n");
        }
        "version" => {
            io.write_str(info.os_name);
            io.write_str(" ");
            io.write_str(info.os_version);
            io.write_str("\n");
        }
        "about" => write_about(io, info),
        "pwd" => {
            io.write_str(info.home_path);
            io.write_str(" (");
            io.write_str(info.home_display);
            io.write_str(")\n");
        }
        "whoami" => {
            io.write_str(info.username);
            io.write_str("\n");
        }
        "uname" => {
            io.write_str("VLR ");
            io.write_str(info.os_version);
            io.write_str(" x86-64\n");
        }
        "ticks" => {
            let mut buf = [0u8; 48];
            let n = format_ticks(info_ticks(io), &mut buf);
            let text = core::str::from_utf8(&buf[..n]).unwrap_or("ticks: ?\n");
            io.write_str(text);
        }
        "mem" => write_mem(io),
        "ps" => write_ps(io),
        "spawn" => match io.spawn_task(rest) {
            Ok(id) => {
                let line = alloc::format!("spawned {id}\n");
                io.write_str(&line);
            }
            Err(err) => {
                io.write_str(err);
                io.write_str("\n");
            }
        },
        "kill" => match parse_u64(rest) {
            Some(id) => match io.kill_task(id) {
                Ok(()) => {
                    let line = alloc::format!("killed {id}\n");
                    io.write_str(&line);
                }
                Err(err) => {
                    io.write_str(err);
                    io.write_str("\n");
                }
            },
            None => io.write_str("kill failed\n"),
        },
        "history" => {
            for (index, command) in history.iter().enumerate() {
                let line = alloc::format!("{index} {command}\n");
                io.write_str(&line);
            }
        }
        _ => io.write_str("unknown command\n"),
    }
}

fn info_ticks<I: ShellIo>(io: &mut I) -> u64 {
    io.ticks()
}

fn format_ticks(ticks: u64, out: &mut [u8]) -> usize {
    struct Buf<'a> {
        bytes: &'a mut [u8],
        used: usize,
    }
    impl Write for Buf<'_> {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            let bytes = text.as_bytes();
            let room = self.bytes.len().saturating_sub(self.used);
            let count = bytes.len().min(room);
            self.bytes[self.used..self.used + count].copy_from_slice(&bytes[..count]);
            self.used += count;
            Ok(())
        }
    }
    let mut buf = Buf {
        bytes: out,
        used: 0,
    };
    let _ = write!(buf, "ticks: {ticks}\n");
    buf.used
}

fn write_about<I: ShellIo>(io: &mut I, info: &ShellInfo) {
    io.write_str(info.os_name);
    io.write_str(" (VLR) ");
    io.write_str(info.os_version);
    io.write_str("\nx86-64 kernel, multitasking milestone\nuser: ");
    io.write_str(info.username);
    io.write_str("\nhome: ");
    io.write_str(info.home_path);
    io.write_str(" (");
    io.write_str(info.home_display);
    io.write_str(")\nshell: in-kernel vch, later /Apps/vch/vch.vlr\n");
    io.write_str("data: /AppData/vch\nlibs: /Libs\nfilesystem: btrfs planned, read-only first\n");
    let mut buf = [0u8; 64];
    let mib = info.usable_memory / (1024 * 1024);
    let n = {
        struct Buf<'a> {
            bytes: &'a mut [u8],
            used: usize,
        }
        impl Write for Buf<'_> {
            fn write_str(&mut self, text: &str) -> core::fmt::Result {
                let bytes = text.as_bytes();
                let room = self.bytes.len().saturating_sub(self.used);
                let count = bytes.len().min(room);
                self.bytes[self.used..self.used + count].copy_from_slice(&bytes[..count]);
                self.used += count;
                Ok(())
            }
        }
        let mut writer = Buf {
            bytes: &mut buf,
            used: 0,
        };
        let _ = write!(writer, "usable memory: {mib} MiB\n");
        writer.used
    };
    let text = core::str::from_utf8(&buf[..n]).unwrap_or("");
    io.write_str(text);
}

fn write_ps<I: ShellIo>(io: &mut I) {
    io.write_str("id name state ticks\n");
    for task in io.tasks() {
        let line = alloc::format!("{} {} {} {}\n", task.id, task.name, task.state, task.ticks);
        io.write_str(&line);
    }
}

fn parse_u64(text: &str) -> Option<u64> {
    if text.is_empty() {
        return None;
    }
    let mut value = 0u64;
    for byte in text.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value
            .saturating_mul(10)
            .saturating_add((byte - b'0') as u64);
    }
    Some(value)
}

fn write_mem<I: ShellIo>(io: &mut I) {
    let memory = io.memory();
    let text = alloc::format!(
        "frames: total {} used {} free {}\nheap: total {} used {} free {}\n",
        memory.frames_total,
        memory.frames_used,
        memory.frames_free,
        memory.heap_total,
        memory.heap_used,
        memory.heap_free
    );
    io.write_str(&text);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MemIo {
        input: &'static [u8],
        pos: usize,
        out: std::string::String,
        cleared: bool,
    }

    impl ShellIo for MemIo {
        fn write_str(&mut self, text: &str) {
            self.out.push_str(text);
        }
        fn clear(&mut self) {
            self.cleared = true;
            self.out.clear();
        }
        fn backspace(&mut self) {
            self.out.pop();
        }
        fn read_byte(&mut self) -> u8 {
            let byte = self.input[self.pos];
            self.pos += 1;
            byte
        }
        fn ticks(&mut self) -> u64 {
            42
        }
        fn memory(&mut self) -> MemorySnapshot {
            MemorySnapshot {
                frames_total: 100,
                frames_used: 40,
                frames_free: 60,
                heap_total: 4096,
                heap_used: 128,
                heap_free: 3968,
            }
        }
        fn tasks(&mut self) -> Vec<TaskSnapshot> {
            alloc::vec![
                TaskSnapshot {
                    id: 0,
                    name: String::from("idle"),
                    state: "running",
                    ticks: 1,
                },
                TaskSnapshot {
                    id: 1,
                    name: String::from("vch"),
                    state: "ready",
                    ticks: 4,
                },
            ]
        }
        fn spawn_task(&mut self, name: &str) -> Result<u64, &'static str> {
            if name == "ticker" {
                Ok(5)
            } else {
                Err("unknown task")
            }
        }
        fn kill_task(&mut self, id: u64) -> Result<(), &'static str> {
            if id == 0 {
                Err("kill failed")
            } else {
                Ok(())
            }
        }
    }

    fn info() -> ShellInfo {
        ShellInfo {
            username: "user",
            os_name: "Veriluvrionikrousdorn",
            os_version: "0.1.0",
            usable_memory: 128 * 1024 * 1024,
            home_display: "~",
            home_path: "/Users/user",
        }
    }

    #[test]
    fn prompt_matches_the_owner_format() {
        let mut buf = [0u8; 64];
        let n = format_prompt("user", "~", &mut buf);
        assert_eq!(&buf[..n], b"user@::vch#~ ");
        let n = format_prompt("user", "/Apps", &mut buf);
        assert_eq!(&buf[..n], b"user@::vch#/Apps ");
    }

    #[test]
    fn line_echoes_and_supports_backspace() {
        let mut io = MemIo {
            input: b"heX\x08lp\n",
            pos: 0,
            out: std::string::String::new(),
            cleared: false,
        };
        let line = read_line(&mut io);
        assert_eq!(line, "help");
        assert_eq!(io.out, "help\n");
    }

    #[test]
    fn builtins_and_unknown_command() {
        let mut io = MemIo {
            input: b"",
            pos: 0,
            out: std::string::String::new(),
            cleared: false,
        };
        let info = info();
        let history = Vec::new();
        dispatch(&mut io, &info, "echo hello vlr", &history);
        assert_eq!(io.out, "hello vlr\n");

        io.out.clear();
        dispatch(&mut io, &info, "version", &history);
        assert_eq!(io.out, "Veriluvrionikrousdorn 0.1.0\n");

        io.out.clear();
        dispatch(&mut io, &info, "pwd", &history);
        assert_eq!(io.out, "/Users/user (~)\n");

        io.out.clear();
        dispatch(&mut io, &info, "ticks", &history);
        assert_eq!(io.out, "ticks: 42\n");

        io.out.clear();
        dispatch(&mut io, &info, "mem", &history);
        assert_eq!(
            io.out,
            "frames: total 100 used 40 free 60\nheap: total 4096 used 128 free 3968\n"
        );

        io.out.clear();
        let history = alloc::vec![String::from("echo hello"), String::from("history")];
        dispatch(&mut io, &info, "history", &history);
        assert_eq!(io.out, "0 echo hello\n1 history\n");

        io.out.clear();
        io.out.clear();
        dispatch(&mut io, &info, "ps", &[]);
        assert!(io.out.contains("id name state ticks"));
        assert!(io.out.contains("idle"));
        assert!(io.out.contains("vch"));

        io.out.clear();
        dispatch(&mut io, &info, "spawn ticker", &[]);
        assert_eq!(io.out, "spawned 5\n");

        io.out.clear();
        dispatch(&mut io, &info, "spawn nope", &[]);
        assert_eq!(io.out, "unknown task\n");

        io.out.clear();
        dispatch(&mut io, &info, "kill 5", &[]);
        assert_eq!(io.out, "killed 5\n");

        io.out.clear();
        dispatch(&mut io, &info, "kill", &[]);
        assert_eq!(io.out, "kill failed\n");

        io.out.clear();
        dispatch(&mut io, &info, "nope", &[]);
        assert_eq!(io.out, "unknown command\n");

        dispatch(&mut io, &info, "help", &[]);
        assert!(io.out.contains("echo"));
        assert!(io.out.contains("mem"));

        dispatch(&mut io, &info, "clear", &[]);
        assert!(io.cleared);

        io.out.clear();
        dispatch(&mut io, &info, "about", &[]);
        assert!(io.out.contains("Veriluvrionikrousdorn"));
        assert!(io.out.contains("/Apps/vch/vch.vlr"));
        assert!(io.out.contains("128 MiB"));
    }
}
