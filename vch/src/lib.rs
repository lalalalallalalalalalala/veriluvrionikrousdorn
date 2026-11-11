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
    pub mode: &'static str,
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
    fn run_user(&mut self, name: &str) -> Result<u64, &'static str>;
    fn current_dir(&mut self) -> String;
    fn change_dir(&mut self, path: &str);
    fn list_dir(&mut self, path: &str) -> Result<Vec<String>, &'static str>;
    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, &'static str>;
    fn exec(&mut self, path: &str) -> Result<u64, &'static str>;
    fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), &'static str> {
        let _ = (path, data);
        Err("read only")
    }
    fn create_file(&mut self, path: &str) -> Result<(), &'static str> {
        let _ = path;
        Err("read only")
    }
    fn make_dir(&mut self, path: &str) -> Result<(), &'static str> {
        let _ = path;
        Err("read only")
    }
    fn remove_path(&mut self, path: &str) -> Result<(), &'static str> {
        let _ = path;
        Err("read only")
    }
    fn rename_path(&mut self, from: &str, to: &str) -> Result<(), &'static str> {
        let _ = (from, to);
        Err("read only")
    }
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
    load_saved(io, &mut history);
    loop {
        let cwd = io.current_dir();
        let shown = if cwd == info.home_path {
            String::from(info.home_display)
        } else {
            cwd
        };
        write_prompt(io, info.username, &shown);
        let line = read_line(io);
        if line.is_empty() {
            continue;
        }
        if history.len() == HISTORY_LIMIT {
            history.remove(0);
        }
        history.push(line.clone());
        dispatch(io, info, &line, &history);
        save_state(io, &history);
    }
}

fn load_saved<I: ShellIo>(io: &mut I, history: &mut Vec<String>) {
    if let Ok(bytes) = io.read_file("/AppData/vch/history") {
        if let Ok(text) = core::str::from_utf8(&bytes) {
            for line in text.lines() {
                if line.is_empty() {
                    continue;
                }
                if history.len() == HISTORY_LIMIT {
                    history.remove(0);
                }
                history.push(String::from(line));
            }
        }
    }
    if let Ok(bytes) = io.read_file("/AppData/vch/settings") {
        if let Ok(text) = core::str::from_utf8(&bytes) {
            for line in text.lines() {
                if let Some(cwd) = line.strip_prefix("cwd=") {
                    if io.list_dir(cwd).is_ok() {
                        io.change_dir(cwd);
                    }
                }
            }
        }
    }
}

fn save_state<I: ShellIo>(io: &mut I, history: &[String]) {
    let mut text = String::new();
    for line in history {
        text.push_str(line);
        text.push('\n');
    }
    let _ = io.write_file("/AppData/vch/history", text.as_bytes());
    let settings = alloc::format!("cwd={}\n", io.current_dir());
    let _ = io.write_file("/AppData/vch/settings", settings.as_bytes());
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
                 kill     stop a task by id\n  \
                 ls       list a directory\n  \
                 cat      print a file\n  \
                 cd       change directory\n  \
                 touch    create an empty file\n  \
                 mkdir    create a directory\n  \
                 rm       remove a file or empty directory\n  \
                 mv       rename a file or directory\n  \
                 write    write text into a file\n  \
                 run      run a .vlr by name or path\n",
            );
        }
        "clear" => io.clear(),
        "echo" => {
            if let Some((text, path, append)) = split_redirect(rest) {
                let path = io_path(io, info, path);
                let mut body = if append {
                    io.read_file(&path).unwrap_or_default()
                } else {
                    Vec::new()
                };
                body.extend_from_slice(text.as_bytes());
                body.push(b'\n');
                let result = io.write_file(&path, &body);
                report(io, result);
            } else {
                io.write_str(rest);
                io.write_str("\n");
            }
        }
        "version" => {
            io.write_str(info.os_name);
            io.write_str(" ");
            io.write_str(info.os_version);
            io.write_str("\n");
        }
        "about" => write_about(io, info),
        "pwd" => {
            let cwd = io.current_dir();
            if cwd == info.home_path {
                io.write_str(info.home_path);
                io.write_str(" (");
                io.write_str(info.home_display);
                io.write_str(")\n");
            } else {
                io.write_str(&cwd);
                io.write_str("\n");
            }
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
        "run" => {
            let path = program_path(io, info, rest);
            launch(io, &path);
        }
        "ls" => {
            let path = if rest.is_empty() {
                io.current_dir()
            } else {
                io_path(io, info, rest)
            };
            match io.list_dir(&path) {
                Ok(names) => {
                    if names.is_empty() {
                        io.write_str("\n");
                    }
                    for name in names {
                        io.write_str(&name);
                        io.write_str("\n");
                    }
                }
                Err(err) => {
                    io.write_str(err);
                    io.write_str("\n");
                }
            }
        }
        "cat" => {
            let path = io_path(io, info, rest);
            match io.read_file(&path) {
                Ok(bytes) => {
                    let text = core::str::from_utf8(&bytes).unwrap_or("?");
                    io.write_str(text);
                    if !text.ends_with('\n') {
                        io.write_str("\n");
                    }
                }
                Err(err) => {
                    io.write_str(err);
                    io.write_str("\n");
                }
            }
        }
        "touch" => {
            let path = io_path(io, info, rest);
            let result = io.create_file(&path);
            report(io, result);
        }
        "mkdir" => {
            let path = io_path(io, info, rest);
            let result = io.make_dir(&path);
            report(io, result);
        }
        "rm" => {
            let path = io_path(io, info, rest);
            let result = io.remove_path(&path);
            report(io, result);
        }
        "mv" => {
            let mut parts = rest.splitn(2, char::is_whitespace);
            let from = parts.next().unwrap_or("");
            let to = parts.next().unwrap_or("").trim();
            if from.is_empty() || to.is_empty() {
                io.write_str("mv failed\n");
            } else {
                let from = io_path(io, info, from);
                let to = io_path(io, info, to);
                let result = io.rename_path(&from, &to);
                report(io, result);
            }
        }
        "write" => {
            let mut parts = rest.splitn(2, char::is_whitespace);
            let path = parts.next().unwrap_or("");
            let text = parts.next().unwrap_or("");
            if path.is_empty() {
                io.write_str("write failed\n");
            } else {
                let path = io_path(io, info, path);
                let mut body = text.as_bytes().to_vec();
                body.push(b'\n');
                let result = io.write_file(&path, &body);
                report(io, result);
            }
        }
        "cd" => {
            let path = if rest.is_empty() {
                String::from(info.home_path)
            } else {
                io_path(io, info, rest)
            };
            match io.list_dir(&path) {
                Ok(_) => io.change_dir(&path),
                Err(err) => {
                    io.write_str(err);
                    io.write_str("\n");
                }
            }
        }
        "history" => {
            for (index, command) in history.iter().enumerate() {
                let line = alloc::format!("{index} {command}\n");
                io.write_str(&line);
            }
        }
        _ => {
            if command.starts_with('/') {
                launch(io, command);
            } else {
                let path = alloc::format!("/Apps/{command}/{command}.vlr");
                match io.exec(&path) {
                    Ok(id) => {
                        let line = alloc::format!("user {id}\n");
                        io.write_str(&line);
                    }
                    Err("rejected") => io.write_str("rejected\n"),
                    Err(_) => io.write_str("unknown command\n"),
                }
            }
        }
    }
}

fn report<I: ShellIo>(io: &mut I, result: Result<(), &'static str>) {
    if let Err(err) = result {
        io.write_str(err);
        io.write_str("\n");
    }
}

fn split_redirect(rest: &str) -> Option<(&str, &str, bool)> {
    if let Some(index) = rest.rfind(" >> ") {
        Some((rest[..index].trim(), rest[index + 4..].trim(), true))
    } else if let Some(index) = rest.rfind(" > ") {
        Some((rest[..index].trim(), rest[index + 3..].trim(), false))
    } else {
        None
    }
}

fn launch<I: ShellIo>(io: &mut I, path: &str) {
    if path.is_empty() {
        io.write_str("unknown program\n");
        return;
    }
    match io.exec(path) {
        Ok(id) => {
            let line = alloc::format!("user {id}\n");
            io.write_str(&line);
        }
        Err(err) => {
            io.write_str(err);
            io.write_str("\n");
        }
    }
}

fn program_path<I: ShellIo>(io: &mut I, info: &ShellInfo, input: &str) -> String {
    if input.starts_with('/') || input.contains('/') {
        let path = io_path(io, info, input);
        if path.ends_with(".vlr") {
            path
        } else {
            let name = path.rsplit('/').next().unwrap_or(&path);
            alloc::format!("{path}/{name}.vlr")
        }
    } else {
        alloc::format!("/Apps/{input}/{input}.vlr")
    }
}

fn io_path<I: ShellIo>(io: &mut I, info: &ShellInfo, input: &str) -> String {
    if input.is_empty() || input == "~" {
        return String::from(info.home_path);
    }
    if let Some(rest) = input.strip_prefix("~/") {
        return alloc::format!("{}/{rest}", info.home_path);
    }
    if input.starts_with('/') {
        return String::from(input);
    }
    let cwd = io.current_dir();
    if cwd == "/" {
        alloc::format!("/{input}")
    } else {
        alloc::format!("{cwd}/{input}")
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
    io.write_str("\nx86-64 kernel, user mode milestone\nuser: ");
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
    io.write_str("id name mode state ticks\n");
    for task in io.tasks() {
        let line = alloc::format!(
            "{} {} {} {} {}\n",
            task.id,
            task.name,
            task.mode,
            task.state,
            task.ticks
        );
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
        cwd: String,
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
                    mode: "kernel",
                    state: "running",
                    ticks: 1,
                },
                TaskSnapshot {
                    id: 1,
                    name: String::from("vch"),
                    mode: "kernel",
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
        fn run_user(&mut self, name: &str) -> Result<u64, &'static str> {
            if name == "hello" {
                Ok(7)
            } else {
                Err("unknown program")
            }
        }
        fn current_dir(&mut self) -> String {
            self.cwd.clone()
        }
        fn change_dir(&mut self, path: &str) {
            self.cwd = String::from(path);
        }
        fn list_dir(&mut self, path: &str) -> Result<Vec<String>, &'static str> {
            match path {
                "/Apps" => Ok(alloc::vec![
                    String::from("badfmt"),
                    String::from("greet"),
                    String::from("hello"),
                    String::from("vch"),
                ]),
                "/Users/user" => Ok(alloc::vec![String::from("motd")]),
                _ => Err("not found"),
            }
        }
        fn read_file(&mut self, path: &str) -> Result<Vec<u8>, &'static str> {
            if path == "/Users/user/motd" {
                Ok(alloc::vec![b'v', b'l', b'r', b'-', b'm', b'o', b't', b'd', b'\n'])
            } else {
                Err("not found")
            }
        }
        fn exec(&mut self, path: &str) -> Result<u64, &'static str> {
            if path.contains("badfmt") {
                Err("rejected")
            } else if path.contains("hello") {
                Ok(7)
            } else {
                Err("unknown program")
            }
        }
    }

    fn io(input: &'static [u8]) -> MemIo {
        MemIo {
            input,
            pos: 0,
            out: std::string::String::new(),
            cleared: false,
            cwd: String::from("/Users/user"),
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
        let mut io = io(b"heX\x08lp\n");
        let line = read_line(&mut io);
        assert_eq!(line, "help");
        assert_eq!(io.out, "help\n");
    }

    #[test]
    fn builtins_and_unknown_command() {
        let mut io = io(b"");
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
        assert!(io.out.contains("id name mode state ticks"));
        assert!(io.out.contains("idle"));
        assert!(io.out.contains("kernel"));
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
        dispatch(&mut io, &info, "run hello", &[]);
        assert_eq!(io.out, "user 7\n");

        io.out.clear();
        dispatch(&mut io, &info, "run nope", &[]);
        assert_eq!(io.out, "unknown program\n");

        io.out.clear();
        dispatch(&mut io, &info, "kill", &[]);
        assert_eq!(io.out, "kill failed\n");

        io.out.clear();
        dispatch(&mut io, &info, "nope", &[]);
        assert_eq!(io.out, "unknown command\n");

        io.out.clear();
        dispatch(&mut io, &info, "ls /Apps", &[]);
        assert!(io.out.contains("vch"));
        assert!(io.out.contains("hello"));

        io.out.clear();
        dispatch(&mut io, &info, "cat /Users/user/motd", &[]);
        assert_eq!(io.out, "vlr-motd\n");

        io.out.clear();
        dispatch(&mut io, &info, "cd /Apps", &[]);
        dispatch(&mut io, &info, "pwd", &[]);
        assert_eq!(io.out, "/Apps\n");

        io.out.clear();
        dispatch(&mut io, &info, "run /Apps/badfmt/badfmt.vlr", &[]);
        assert_eq!(io.out, "rejected\n");

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
