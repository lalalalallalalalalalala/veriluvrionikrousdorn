//! Один пользователь на машину: первый запуск создаёт его, дальше вход.

use alloc::string::String;

use vlr_account::{home_of, parse, seal, Account, ACCOUNT_PATH};
use vlr_drivers::{display, input, serial};

pub fn establish() -> String {
    if let Some(account) = load() {
        if account.password_required() {
            let title = alloc::format!("Welcome, {}", account.name);
            loop {
                let password = prompt(&title, "password", true);
                if account.verify(&password) {
                    break;
                }
                serial::write_str("wrong password\n");
            }
        }
        vlr_btrfs::set_session(&account.name);
        return account.name;
    }
    let account = create();
    let name = account.name.clone();
    if vlr_btrfs::system_mkdir("/System").is_err() {
        serial::write_str("account: system dir failed\n");
    }
    let home = home_of(&name);
    if vlr_btrfs::system_mkdir("/Users").is_err() {
        serial::write_str("account: users dir failed\n");
    }
    if vlr_btrfs::system_mkdir(&home).is_err() {
        serial::write_str("account: home failed\n");
    }
    let encoded = vlr_account::encode(&account);
    if vlr_btrfs::system_write(ACCOUNT_PATH, encoded.as_bytes()).is_err() {
        serial::write_str("account: save failed\n");
    }
    vlr_btrfs::set_session(&name);
    serial::write_str("session ");
    serial::write_str(&name);
    serial::write_str("\n");
    name
}

fn load() -> Option<Account> {
    let bytes = vlr_vfs::read_file(ACCOUNT_PATH).ok()?;
    let text = core::str::from_utf8(&bytes).ok()?;
    parse(text)
}

fn create() -> Account {
    serial::write_str("first boot\n");
    let name = loop {
        let name = prompt("Create the only account", "name", false);
        if vlr_account::valid_name(&name) {
            break name;
        }
        serial::write_str("use 1-16 letters or digits\n");
    };
    let (password, _) = loop {
        let first = prompt("Create the only account", "password", true);
        let second = prompt("Create the only account", "repeat", true);
        if first == second {
            break (first, second);
        }
        serial::write_str("passwords differ\n");
    };
    let ticks = vlr_drivers::timer::ticks().to_le_bytes();
    let clock = vlr_drivers::rtc::read();
    let salt = [
        ticks[0], ticks[1], ticks[2], ticks[3], ticks[4], ticks[5], clock.second, clock.minute,
    ];
    seal(&name, &password, salt)
}

fn prompt(title: &str, label: &str, secret: bool) -> String {
    if display::is_ready() {
        vlr_gui::ask(title, label, secret)
    } else {
        serial::write_str(label);
        serial::write_str(": ");
        let mut line = String::new();
        loop {
            match input::read_byte() {
                b'\n' => {
                    serial::write_str("\n");
                    return line;
                }
                0x08 | 0x7F => {
                    line.pop();
                }
                byte if (0x20..0x7F).contains(&byte) && line.len() < 32 => {
                    line.push(byte as char);
                    if !secret {
                        let text = [byte];
                        serial::write_str(core::str::from_utf8(&text).unwrap_or(""));
                    }
                }
                _ => {}
            }
        }
    }
}
