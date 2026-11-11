//! Встроенная оболочка vch.
//!
//! Это временный дом программы. Позже тот же цикл «приглашение, строка, команда»
//! переедет в `/Apps/vch/vch.vlr` и будет говорить с ядром через системные вызовы.
//! Сейчас ввод и вывод приходят через [`ShellIo`], чтобы модуль не зависел от
//! драйверов и его можно было проверить на хосте.

#![cfg_attr(not(test), no_std)]

use core::fmt::Write;

pub trait ShellIo {
    fn write_str(&mut self, text: &str);
    fn clear(&mut self);
    fn backspace(&mut self);
    fn read_byte(&mut self) -> u8;
    fn ticks(&mut self) -> u64;
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
    loop {
        write_prompt(io, info.username, info.home_display);
        let mut raw = [0u8; 256];
        let len = read_line(io, &mut raw);
        let line = core::str::from_utf8(&raw[..len]).unwrap_or("");
        if line.is_empty() {
            continue;
        }
        dispatch(io, info, line);
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

pub fn read_line<I: ShellIo>(io: &mut I, buf: &mut [u8]) -> usize {
    let mut len = 0;
    loop {
        match io.read_byte() {
            b'\n' | b'\r' => {
                io.write_str("\n");
                return len;
            }
            0x08 | 0x7F => {
                if len > 0 {
                    len -= 1;
                    io.backspace();
                }
            }
            byte if (0x20..0x7F).contains(&byte) => {
                if len < buf.len() {
                    buf[len] = byte;
                    len += 1;
                    let text = [byte];
                    io.write_str(core::str::from_utf8(&text).unwrap_or(""));
                }
            }
            _ => {}
        }
    }
}

pub fn dispatch<I: ShellIo>(io: &mut I, info: &ShellInfo, line: &str) {
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
                 ticks    hardware timer ticks\n",
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
    io.write_str("\nx86-64 kernel, foundation milestone\nuser: ");
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
        let mut buf = [0u8; 32];
        let n = read_line(&mut io, &mut buf);
        assert_eq!(&buf[..n], b"help");
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
        dispatch(&mut io, &info, "echo hello vlr");
        assert_eq!(io.out, "hello vlr\n");

        io.out.clear();
        dispatch(&mut io, &info, "version");
        assert_eq!(io.out, "Veriluvrionikrousdorn 0.1.0\n");

        io.out.clear();
        dispatch(&mut io, &info, "pwd");
        assert_eq!(io.out, "/Users/user (~)\n");

        io.out.clear();
        dispatch(&mut io, &info, "ticks");
        assert_eq!(io.out, "ticks: 42\n");

        io.out.clear();
        dispatch(&mut io, &info, "nope");
        assert_eq!(io.out, "unknown command\n");

        dispatch(&mut io, &info, "help");
        assert!(io.out.contains("echo"));

        dispatch(&mut io, &info, "clear");
        assert!(io.cleared);

        io.out.clear();
        dispatch(&mut io, &info, "about");
        assert!(io.out.contains("Veriluvrionikrousdorn"));
        assert!(io.out.contains("/Apps/vch/vch.vlr"));
        assert!(io.out.contains("128 MiB"));
    }
}
