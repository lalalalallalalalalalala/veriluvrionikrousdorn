//! Собрать ядро, упаковать BIOS-образ и запустить QEMU.
//!
//! `cargo run` попадает сюда. Вложенный `cargo build` пишет в отдельный
//! каталог, чтобы не ждать блокировку `target/` родительского cargo.

use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use bootloader::DiskImageBuilder;

const SERIAL_PORT: u16 = 18765;
const MONITOR_PORT: u16 = 18766;

fn main() {
    if let Err(error) = run() {
        eprintln!("xtask: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let gui = args.iter().any(|arg| arg == "--gui");
    let command = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map(String::as_str)
        .unwrap_or("run");

    match command {
        "usb" => {
            build_userspace()?;
            make_filesystem()?;
            build_kernel(false)?;
            make_live()?;
        }
        "build" => {
            build_userspace()?;
            make_filesystem()?;
            build_kernel(false)?;
            make_image()?;
            println!("image: {}", image_path().display());
            println!("disk: {}", disk_path().display());
        }
        "run" => {
            build_userspace()?;
            make_filesystem()?;
            build_kernel(false)?;
            make_image()?;
            run_qemu(gui)?;
        }
        "test" => {
            host_tests()?;
            account_test()?;
            persist_test()?;
            boot_test()?;
            shell_test()?;
            usb_test()?;
            input_test()?;
            live_test()?;
            println!("vlr tests passed");
        }
        other => bail!("unknown command '{other}'. expected build, run, or test"),
    }
    Ok(())
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace")
        .to_path_buf()
}

fn kernel_target_dir() -> PathBuf {
    workspace_root().join("target/kernel-build")
}

fn kernel_elf(boot_test: bool) -> PathBuf {
    let profile = "debug";
    let _ = boot_test;
    kernel_target_dir()
        .join("x86_64-unknown-none")
        .join(profile)
        .join("kernel")
}

fn image_path() -> PathBuf {
    workspace_root().join("target/vlr-bios.img")
}

fn disk_path() -> PathBuf {
    workspace_root().join("target/vlr-btrfs.img")
}

fn cargo() -> Command {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(&cargo);
    for (key, _) in env::vars() {
        if key.starts_with("CARGO_") {
            command.env_remove(key);
        }
    }
    command.env("CARGO_TARGET_DIR", kernel_target_dir());
    command.current_dir(workspace_root());
    command
}

fn build_kernel(boot_test: bool) -> Result<()> {
    let mut command = cargo();
    command.args(["build", "-p", "kernel", "--target", "x86_64-unknown-none"]);
    if boot_test {
        command.args(["--no-default-features", "--features", "boot-test"]);
    }
    let status = command.status().context("failed to spawn cargo build")?;
    if !status.success() {
        bail!("kernel build failed");
    }
    if !kernel_elf(boot_test).is_file() {
        bail!(
            "kernel ELF is missing at {}",
            kernel_elf(boot_test).display()
        );
    }
    Ok(())
}

fn make_live() -> Result<()> {
    let elf = kernel_elf(false);
    let bios = workspace_root().join("target/vlr-live.img");
    let uefi = workspace_root().join("target/vlr-live-uefi.img");
    let mut bios_builder = DiskImageBuilder::new(elf.clone());
    bios_builder
        .set_ramdisk(disk_path())
        .create_bios_image(&bios)
        .context("BIOS live image")?;
    let mut uefi_builder = DiskImageBuilder::new(elf);
    uefi_builder
        .set_ramdisk(disk_path())
        .create_uefi_image(&uefi)
        .context("UEFI live image")?;
    println!("live bios: {}", bios.display());
    println!("live uefi: {}", uefi.display());
    Ok(())
}

fn usb_test() -> Result<()> {
    build_userspace()?;
    make_filesystem()?;
    build_kernel(false)?;
    make_image()?;
    let stick = workspace_root().join("target/vlr-usb-root.img");
    fs::copy(disk_path(), &stick)?;
    run_usb_boot(&stick, true)?;
    let check = Command::new("btrfs")
        .args(["check", "--readonly", stick.to_str().context("usb image")?])
        .status()
        .context("btrfs check usb")?;
    if !check.success() {
        bail!("btrfs check rejected the USB image after the write");
    }
    run_usb_boot(&stick, false)?;
    println!("usb msc test passed");
    Ok(())
}

fn run_usb_boot(stick: &std::path::Path, writing: bool) -> Result<()> {
    let serial_port = 18781u16;
    let monitor_port = 18782u16;
    let mut child = Command::new("qemu-system-x86_64")
        .args([
            "-machine", "pc", "-accel", "tcg", "-cpu", "qemu64", "-smp", "1", "-m", "256M",
            "-display", "none", "-no-reboot",
            "-device", "qemu-xhci,id=xhci",
        ])
        .arg("-drive")
        .arg(format!("format=raw,if=ide,index=0,file={}", image_path().display()))
        .arg("-drive")
        .arg(format!(
            "if=none,id=stick,format=raw,cache=writethrough,file={}",
            stick.display()
        ))
        .args(["-device", "usb-storage,bus=xhci.0,drive=stick"])
        .arg("-serial")
        .arg(format!("tcp:127.0.0.1:{serial_port},server"))
        .arg("-monitor")
        .arg(format!("tcp:127.0.0.1:{monitor_port},server,nowait"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("usb qemu")?;
    let result = (|| {
        let serial = connect_retry(("127.0.0.1", serial_port), Duration::from_secs(20))?;
        let mut writer = serial.try_clone()?;
        let captured = Arc::new(Mutex::new(String::new()));
        let reader = Arc::clone(&captured);
        thread::spawn(move || {
            let mut serial = serial;
            let mut buf = [0u8; 256];
            loop {
                match serial.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => reader
                        .lock()
                        .expect("serial")
                        .push_str(&String::from_utf8_lossy(&buf[..n])),
                }
            }
        });
        for needle in ["usb-ok", "usb-root", "btrfs-ok", "user@::vch#~ "] {
            if !wait_for(&captured, needle, Duration::from_secs(40)) {
                bail!("usb boot missing '{needle}':\n{}", captured.lock().unwrap());
            }
        }
        if writing {
            writer.write_all(b"write /Users/user/usbkept usb-persist-ok\rcat /Users/user/usbkept\r")?;
            writer.flush().ok();
            if !wait_count(&captured, "usb-persist-ok", 2, Duration::from_secs(25)) {
                bail!("usb write missing:\n{}", captured.lock().unwrap());
            }
        } else {
            writer.write_all(b"cat /Users/user/usbkept\r")?;
            writer.flush().ok();
            if !wait_for(&captured, "usb-persist-ok", Duration::from_secs(40)) {
                bail!("usb file did not survive reboot:\n{}", captured.lock().unwrap());
            }
        }
        Ok(())
    })();
    if result.is_ok() {
        if let Ok(mut monitor) = connect_retry(("127.0.0.1", monitor_port), Duration::from_secs(5)) {
            let _ = write!(monitor, "quit\n");
            let _ = monitor.flush();
            let _ = child.wait();
            return result;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn input_test() -> Result<()> {
    build_userspace()?;
    make_filesystem()?;
    build_kernel(false)?;
    make_image()?;
    let serial_port = 18783u16;
    let qmp_port = 18784u16;
    let mut child = Command::new("qemu-system-x86_64")
        .args([
            "-machine", "pc", "-accel", "tcg", "-cpu", "qemu64", "-smp", "1", "-m", "256M",
            "-display", "none", "-vga", "std", "-no-reboot",
            "-device", "qemu-xhci,id=xhci",
            "-device", "usb-kbd,id=kbd",
            "-device", "usb-mouse,id=mouse",
            "-device", "usb-tablet,id=tablet",
        ])
        .arg("-drive")
        .arg(format!("format=raw,if=ide,index=0,file={}", image_path().display()))
        .arg("-drive")
        .arg(format!(
            "format=raw,if=ide,index=1,cache=writethrough,file={}",
            disk_path().display()
        ))
        .arg("-serial")
        .arg(format!("tcp:127.0.0.1:{serial_port},server"))
        .arg("-qmp")
        .arg(format!("tcp:127.0.0.1:{qmp_port},server,nowait"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("input qemu")?;
    let result = (|| {
        let serial = connect_retry(("127.0.0.1", serial_port), Duration::from_secs(20))?;
        let mut writer = serial.try_clone()?;
        let captured = Arc::new(Mutex::new(String::new()));
        let reader = Arc::clone(&captured);
        thread::spawn(move || {
            let mut serial = serial;
            let mut buf = [0u8; 256];
            loop {
                match serial.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => reader.lock().expect("serial").push_str(&String::from_utf8_lossy(&buf[..n])),
                }
            }
        });
        if !wait_for(&captured, "user@::vch#~ ", Duration::from_secs(40)) {
            bail!("input prompt missing:\n{}", captured.lock().unwrap());
        }
        writer.write_all(b"devices\rdisplay\r")?;
        writer.flush().ok();
        for needle in ["usb-keyboard", "usb-mouse", "usb-tablet", "bga-display", "mode "] {
            if !wait_for(&captured, needle, Duration::from_secs(15)) {
                bail!("input devices missing '{needle}':\n{}", captured.lock().unwrap());
            }
        }
        let mut qmp = connect_retry(("127.0.0.1", qmp_port), Duration::from_secs(5))?;
        let mut banner = [0u8; 256];
        let _ = qmp.read(&mut banner);
        qmp.write_all(b"{\"execute\":\"qmp_capabilities\"}\n")?;
        qmp.flush().ok();
        thread::sleep(Duration::from_millis(100));
        qmp.write_all(br#"{"execute":"input-send-event","arguments":{"events":[{"type":"abs","data":{"axis":"x","value":16384}},{"type":"abs","data":{"axis":"y","value":8192}}]}}"#)?;
        qmp.write_all(b"\n")?;
        qmp.flush().ok();
        if !wait_for(&captured, "ptr ", Duration::from_secs(8)) {
            bail!("tablet did not move the cursor:\n{}", captured.lock().unwrap());
        }
        qmp.write_all(br#"{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"down":true,"button":"wheel-up"}},{"type":"btn","data":{"down":false,"button":"wheel-up"}}]}}"#)?;
        qmp.write_all(b"\n")?;
        qmp.flush().ok();
        if !wait_for(&captured, "scroll ", Duration::from_secs(8)) {
            bail!("wheel did not scroll:\n{}", captured.lock().unwrap());
        }
        qmp.write_all(br#"{"execute":"input-send-event","arguments":{"events":[{"type":"key","data":{"down":true,"key":{"type":"qcode","data":"q"}}},{"type":"key","data":{"down":false,"key":{"type":"qcode","data":"q"}}}]}}"#)?;
        qmp.write_all(b"\n")?;
        qmp.flush().ok();
        if !wait_for(&captured, "hid-key q", Duration::from_secs(8)) {
            bail!("usb key did not reach the desktop:\n{}", captured.lock().unwrap());
        }
        // q попала в строку терминала. Закрыть её, потом сменить режим.
        writer.write_all(b"\rdisplay set 800 600\r")?;
        writer.flush().ok();
        if !wait_for(&captured, "mode 800x600", Duration::from_secs(8)) {
            bail!("resolution change failed:\n{}", captured.lock().unwrap());
        }
        Ok(())
    })();
    let _ = child.kill();
    let _ = child.wait();
    if result.is_ok() {
        println!("input test passed");
    }
    result
}

fn live_test() -> Result<()> {
    build_userspace()?;
    make_filesystem()?;
    build_kernel(false)?;
    make_live()?;
    let log_path = workspace_root().join("target/live-test.log");
    if log_path.exists() {
        fs::remove_file(&log_path)?;
    }
    let status = Command::new("timeout")
        .args(["--foreground", "90", "qemu-system-x86_64"])
        .args(["-machine", "pc", "-accel", "tcg", "-cpu", "qemu64", "-m", "512M"])
        .args(["-device", "qemu-xhci,id=xhci"])
        .arg("-drive")
        .arg(format!(
            "if=none,id=stick,format=raw,file={}",
            workspace_root().join("target/vlr-live.img").display()
        ))
        .args(["-device", "usb-storage,bus=xhci.0,drive=stick"])
        .args(["-display", "none", "-no-reboot"])
        .arg("-serial")
        .arg(format!("file:{}", log_path.display()))
        .status()
        .context("live qemu")?;
    let _ = status;
    let log = fs::read_to_string(&log_path).unwrap_or_default();
    for needle in ["live-ok", "btrfs-ok", "gui-ok", "user@::vch#~ "] {
        if !log.contains(needle) {
            eprintln!("----- live serial -----\n{log}\n----- end -----");
            bail!("live USB boot is missing '{needle}'");
        }
    }
    println!("live usb test passed");
    Ok(())
}

fn make_image() -> Result<()> {
    let elf = kernel_elf(false);
    let image = image_path();
    if let Some(parent) = image.parent() {
        fs::create_dir_all(parent)?;
    }
    DiskImageBuilder::new(elf)
        .create_bios_image(&image)
        .context("failed to create the BIOS disk image")?;
    Ok(())
}

fn run_qemu(gui: bool) -> Result<()> {
    let mut command = qemu(gui, false);
    command.arg("-serial").arg("stdio");
    if gui {
        // Окно QEMU получает PS/2, этот терминал остаётся последовательной консолью.
    } else {
        command.args(["-display", "none"]);
    }
    let status = command.status().context("failed to spawn qemu")?;
    if !status.success() {
        bail!("qemu exited with {status}");
    }
    Ok(())
}

fn qemu(gui: bool, test_exit: bool) -> Command {
    let _ = gui;
    let mut command = Command::new("qemu-system-x86_64");
    command
        .arg("-machine")
        .arg("pc")
        .arg("-accel")
        .arg("tcg")
        .arg("-cpu")
        .arg("qemu64")
        .arg("-smp")
        .arg("1")
        .arg("-m")
        .arg("256M")
        .arg("-drive")
        .arg(format!(
            "format=raw,if=ide,index=0,file={}",
            image_path().display()
        ))
        .arg("-drive")
        .arg(format!(
            "format=raw,if=ide,index=1,cache=writethrough,file={}",
            disk_path().display()
        ))
        .arg("-no-reboot");
    if test_exit {
        command
            .arg("-device")
            .arg("isa-debug-exit,iobase=0xf4,iosize=0x04");
    }
    command
}

fn host_tests() -> Result<()> {
    let mut command = cargo();
    command.env(
        "CARGO_TARGET_DIR",
        workspace_root().join("target/host-test"),
    );
    command.args([
        "test",
        "-p",
        "vlr-account",
        "-p",
        "vch",
        "-p",
        "vlr-loader",
        "-p",
        "vlr-memory",
        "-p",
        "vlr-libc",
        "-p",
        "vlr-btrfs",
    ]);
    let status = command.status().context("failed to spawn cargo test")?;
    if !status.success() {
        bail!("host tests failed");
    }
    Ok(())
}

fn boot_test() -> Result<()> {
    build_userspace()?;
    make_filesystem()?;
    build_kernel(true)?;
    make_image()?;
    let log_path = workspace_root().join("target/boot-test.log");
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if log_path.exists() {
        fs::remove_file(&log_path)?;
    }
    let mut command = Command::new("timeout");
    command
        .arg("--foreground")
        .arg("90")
        .arg("qemu-system-x86_64")
        .arg("-machine")
        .arg("pc")
        .arg("-accel")
        .arg("tcg")
        .arg("-cpu")
        .arg("qemu64")
        .arg("-smp")
        .arg("1")
        .arg("-m")
        .arg("256M")
        .arg("-drive")
        .arg(format!(
            "format=raw,if=ide,index=0,file={}",
            image_path().display()
        ))
        .arg("-drive")
        .arg(format!(
            "format=raw,if=ide,index=1,cache=writethrough,file={}",
            disk_path().display()
        ))
        .arg("-display")
        .arg("none")
        .arg("-serial")
        .arg(format!("file:{}", log_path.display()))
        .arg("-no-reboot")
        .arg("-device")
        .arg("isa-debug-exit,iobase=0xf4,iosize=0x04");
    let status = command.status().context("failed to spawn qemu boot test")?;
    let log = fs::read_to_string(&log_path).unwrap_or_default();
    let code = status.code().unwrap_or(-1);
    if code != 33 {
        eprintln!("----- serial log -----\n{log}\n----- end -----");
        bail!("boot test qemu exit {code}, expected 33 (isa-debug-exit success)");
    }
    for needle in [
        "Veriluvrionikrousdorn",
        "VLR kernel 1.3.0",
        "btrfs-ok",
        "motd-ok",
        "apps-ok",
        "vch-loaded",
        "user@::vch#~ ",
        "ls /Apps",
        "badfmt",
        "vlr-motd",
        "rejected",
        "disk-shell-ok",
        "timer-ok",
        "offset-map-ok",
        "heap-large-ok",
        "heap-small-ok",
        "heap-ok",
        "page-map-ok",
        "page-identity-ok",
        "sched-ok",
        "sleep-ok",
        "reap-ok",
        "hello from ring 3",
        "user-hello-ok",
        "user fault",
        "user-bad-ok",
        "exception: page fault at 0xffff820000000000",
        "page-fault-ok addr=0xffff820000000000",
        "vlr-boot-ok",
    ] {
        if !log.contains(needle) {
            eprintln!("----- serial log -----\n{log}\n----- end -----");
            bail!("boot serial log is missing '{needle}'");
        }
    }
    println!("boot test passed");
    Ok(())
}

fn persist_test() -> Result<()> {
    build_userspace()?;
    make_filesystem()?;
    build_kernel(false)?;
    make_image()?;
    let persist = workspace_root().join("target/vlr-btrfs-persist.img");
    fs::copy(disk_path(), &persist)?;
    let bios = image_path();
    run_persist_boot(&bios, &persist, true)?;
    let check = Command::new("btrfs")
        .args(["check", "--readonly", persist.to_str().context("persist path")?])
        .status()
        .context("btrfs check")?;
    if !check.success() {
        bail!("btrfs check rejected the image after the kernel wrote it");
    }
    run_persist_boot(&bios, &persist, false)?;
    println!("persist test passed");
    Ok(())
}

fn run_persist_boot(bios: &std::path::Path, disk: &std::path::Path, writing: bool) -> Result<()> {
    let serial_port = 18771u16;
    let monitor_port = 18772u16;
    let mut child = Command::new("qemu-system-x86_64")
        .arg("-machine").arg("pc")
        .arg("-accel").arg("tcg")
        .arg("-cpu").arg("qemu64")
        .arg("-smp").arg("1")
        .arg("-m").arg("256M")
        .arg("-drive").arg(format!("format=raw,if=ide,index=0,file={}", bios.display()))
        .arg("-drive").arg(format!("format=raw,if=ide,index=1,cache=writethrough,file={}", disk.display()))
        .arg("-display").arg("none")
        .arg("-serial").arg(format!("tcp:127.0.0.1:{serial_port},server"))
        .arg("-monitor").arg(format!("tcp:127.0.0.1:{monitor_port},server,nowait"))
        .arg("-no-reboot")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(fs::File::create(workspace_root().join("target/persist-qemu.log"))?)
        .spawn()
        .context("persist qemu")?;
    let result = (|| {
        let serial = connect_retry(("127.0.0.1", serial_port), Duration::from_secs(15))?;
        let mut writer = serial.try_clone()?;
        let captured = Arc::new(Mutex::new(String::new()));
        let captured_reader = Arc::clone(&captured);
        thread::spawn(move || {
            let mut serial = serial;
            let mut buf = [0u8; 256];
            loop {
                match serial.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buf[..n]);
                        captured_reader.lock().expect("serial").push_str(&text);
                    }
                }
            }
        });
        if !wait_for(&captured, "user@::vch#~ ", Duration::from_secs(30)) {
            bail!("persist prompt missing:\n{}", captured.lock().unwrap());
        }
        if writing {
            writer.write_all(b"write /Users/user/kept persist-ok\rcat /Users/user/kept\r")?;
            writer.flush().ok();
            if !wait_count(&captured, "persist-ok", 2, Duration::from_secs(20)) {
                bail!("written file missing:\n{}", captured.lock().unwrap());
            }
        } else {
            writer.write_all(b"history\r")?;
            writer.flush().ok();
            if !wait_for(&captured, "write /Users/user/kept", Duration::from_secs(10)) {
                bail!("history did not survive reboot:\n{}", captured.lock().unwrap());
            }
            writer.write_all(b"cat /Users/user/kept\r")?;
            writer.flush().ok();
            if !wait_for(&captured, "persist-ok", Duration::from_secs(10)) {
                bail!("file did not survive reboot:\n{}", captured.lock().unwrap());
            }
            writer.write_all(b"cat /AppData/vch/settings\r")?;
            writer.flush().ok();
            if !wait_for(&captured, "cwd=", Duration::from_secs(10)) {
                bail!("settings did not survive reboot:\n{}", captured.lock().unwrap());
            }
        }
        Ok(())
    })();
    if result.is_ok() {
        if let Ok(mut monitor) = connect_retry(("127.0.0.1", monitor_port), Duration::from_secs(5)) {
            let _ = write!(monitor, "quit\n");
            let _ = monitor.flush();
            let _ = child.wait();
            return result;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn shell_test() -> Result<()> {
    build_userspace()?;
    make_filesystem()?;
    build_kernel(false)?;
    make_image()?;
    let stderr_path = workspace_root().join("target/shell-test-qemu.log");
    let stderr = fs::File::create(&stderr_path)?;
    let mut child = qemu(false, false)
        .arg("-display")
        .arg("none")
        .arg("-serial")
        .arg(format!("tcp:127.0.0.1:{SERIAL_PORT},server"))
        .arg("-monitor")
        .arg(format!("tcp:127.0.0.1:{MONITOR_PORT},server,nowait"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .context("failed to spawn qemu for the shell test")?;

    let result = shell_session();
    let _ = child.kill();
    let _ = child.wait();
    if let Err(error) = result {
        if let Ok(qemu_log) = fs::read_to_string(&stderr_path) {
            eprintln!("----- qemu stderr -----\n{qemu_log}\n----- end -----");
        }
        return Err(error);
    }
    println!("shell test passed");
    Ok(())
}

fn shell_session() -> Result<()> {
    let serial = connect_retry(("127.0.0.1", SERIAL_PORT), Duration::from_secs(10))
        .context("serial port")?;
    let mut writer = serial.try_clone().context("clone serial")?;
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_reader = Arc::clone(&captured);
    thread::spawn(move || {
        let mut serial = serial;
        let mut buf = [0u8; 256];
        loop {
            match serial.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]);
                    captured_reader
                        .lock()
                        .expect("serial buffer")
                        .push_str(&text);
                }
            }
        }
    });

    if !wait_for(&captured, "gui-ok", Duration::from_secs(30)) {
        bail!("gui did not start:\n{}", captured.lock().unwrap());
    }
    if !wait_for(&captured, "user@::vch#~ ", Duration::from_secs(30)) {
        bail!("prompt did not appear:\n{}", captured.lock().unwrap());
    }
    writer.write_all(b"help\r").context("write help")?;
    writer.flush().ok();
    if !wait_for(&captured, "user@::vch#~ help", Duration::from_secs(5)) {
        bail!("echoed help line missing:\n{}", captured.lock().unwrap());
    }
    if !wait_for(&captured, "commands:", Duration::from_secs(5)) {
        bail!("help output missing:\n{}", captured.lock().unwrap());
    }

    writer.write_all(b"echo hello\r").context("write echo")?;
    writer.flush().ok();
    if !wait_for(&captured, "hello", Duration::from_secs(5)) {
        bail!("echo output missing:\n{}", captured.lock().unwrap());
    }

    writer.write_all(b"history\r").context("write history")?;
    writer.flush().ok();
    if !wait_for(&captured, "1 echo hello", Duration::from_secs(5)) {
        bail!("history missing echo:\n{}", captured.lock().unwrap());
    }

    writer.write_all(b"mem\r").context("write mem")?;
    writer.flush().ok();
    if !wait_for(&captured, "frames: total", Duration::from_secs(5))
        || !wait_for(&captured, "heap: total", Duration::from_secs(5))
    {
        bail!("mem output missing:\n{}", captured.lock().unwrap());
    }

    writer
        .write_all(b"run greet\r")
        .context("write run greet")?;
    writer.flush().ok();
    if !wait_for(&captured, "greet from ring 3", Duration::from_secs(8)) {
        bail!("user greet missing:\n{}", captured.lock().unwrap());
    }

    writer.write_all(b"ps\r").context("write ps")?;
    writer.flush().ok();
    if !wait_for(&captured, "ticker", Duration::from_secs(5))
        || !wait_for(&captured, "spinner", Duration::from_secs(5))
        || !wait_for(&captured, "greet", Duration::from_secs(5))
        || !wait_for(&captured, "user", Duration::from_secs(5))
    {
        bail!("ps output missing:\n{}", captured.lock().unwrap());
    }

    writer.write_all(b"bogus\r").context("write bogus")?;
    writer.flush().ok();
    if !wait_for(&captured, "unknown command", Duration::from_secs(5)) {
        bail!("unknown command missing:\n{}", captured.lock().unwrap());
    }

    let mut monitor = connect_retry(("127.0.0.1", MONITOR_PORT), Duration::from_secs(5))
        .context("qemu monitor")?;
    // Дать монитору напечатать приглашение, затем набрать pwd с PS/2.
    thread::sleep(Duration::from_millis(100));
    for key in ["p", "w", "d", "ret"] {
        write!(monitor, "sendkey {key}\n").context("sendkey")?;
        monitor.flush().ok();
        thread::sleep(Duration::from_millis(50));
    }
    // greet ещё печатает и иногда вклинивается между приглашением и pwd.
    let echo_deadline = Instant::now() + Duration::from_secs(5);
    let mut echoed = false;
    while Instant::now() < echo_deadline {
        let text = captured.lock().expect("serial");
        echoed = text.contains("user@::vch#~ pwd")
            || text.contains("\npwd\n")
            || text.contains("\npwd\r");
        drop(text);
        if echoed {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    if !echoed {
        bail!(
            "keyboard echo of pwd missing:\n{}",
            captured.lock().unwrap()
        );
    }
    if !wait_for(&captured, "/Users/user (~)", Duration::from_secs(5)) {
        bail!("pwd output missing:\n{}", captured.lock().unwrap());
    }

    writer.write_all(b"ls /Apps\r").context("write ls")?;
    writer.flush().ok();
    if !wait_for(&captured, "badfmt", Duration::from_secs(5))
        || !wait_for(&captured, "vch", Duration::from_secs(5))
    {
        bail!("ls output missing:\n{}", captured.lock().unwrap());
    }
    writer
        .write_all(b"cat /Users/user/motd\r")
        .context("write cat")?;
    writer.flush().ok();
    if !wait_for(&captured, "vlr-motd", Duration::from_secs(5)) {
        bail!("cat output missing:\n{}", captured.lock().unwrap());
    }

    writer
        .write_all(b"install /Users/user/stage/widget.vlr\r")
        .context("install")?;
    writer.flush().ok();
    if !wait_for(&captured, "installed widget", Duration::from_secs(15)) {
        bail!("install failed:\n{}", captured.lock().unwrap());
    }
    writer.write_all(b"uninstall widget\r").context("uninstall")?;
    writer.flush().ok();
    if !wait_for(&captured, "removed widget", Duration::from_secs(15)) {
        bail!("uninstall failed:\n{}", captured.lock().unwrap());
    }
    writer
        .write_all(b"install /Apps/badfmt/badfmt.vlr\r")
        .context("bad install")?;
    writer.flush().ok();
    if !wait_for(&captured, "rejected", Duration::from_secs(10)) {
        bail!("corrupt vlr was installed:\n{}", captured.lock().unwrap());
    }

    writer.write_all(b"run keys\r").context("run keys")?;
    writer.flush().ok();
    if !wait_for(&captured, "keys-open", Duration::from_secs(15)) {
        bail!("keys window missing:\n{}", captured.lock().unwrap());
    }
    writer.write_all(b"Z\r").context("key to window")?;
    writer.flush().ok();
    if !wait_for(&captured, "keys:Z", Duration::from_secs(10)) {
        bail!("focus did not route the key:\n{}", captured.lock().unwrap());
    }
    writer.write_all(&[0x0e]).context("cycle focus")?;
    writer.flush().ok();
    writer.write_all(b"echo back\r").context("echo after focus")?;
    writer.flush().ok();
    if !wait_for(&captured, "back", Duration::from_secs(10)) {
        bail!("focus did not return to vch:\n{}", captured.lock().unwrap());
    }

    writer.write_all(&[0x0c]).context("lock")?;
    writer.flush().ok();
    if !wait_for(&captured, "password:", Duration::from_secs(10)) {
        bail!("lock screen missing:\n{}", captured.lock().unwrap());
    }
    writer.write_all(b"\r").context("unlock")?;
    writer.flush().ok();
    if !wait_for(&captured, "unlocked", Duration::from_secs(10)) {
        bail!("empty password did not unlock:\n{}", captured.lock().unwrap());
    }
    Ok(())
}

fn build_userspace() -> Result<()> {
    let target = workspace_root().join("target/user-build");
    for package in ["hello", "greet", "vch-bin", "keys"] {
        let mut command = cargo();
        command.env("CARGO_TARGET_DIR", &target);
        command.args([
            "build",
            "-p",
            package,
            "--target",
            "x86_64-unknown-none",
        ]);
        let status = command.status().context("failed to spawn userspace build")?;
        if !status.success() {
            bail!("{package} build failed");
        }
    }
    Ok(())
}

fn make_filesystem() -> Result<()> {
    make_fs(true)
}

fn account_test() -> Result<()> {
    build_userspace()?;
    make_fs(false)?;
    build_kernel(false)?;
    make_image()?;
    let image = workspace_root().join("target/vlr-btrfs-newuser.img");
    fs::copy(disk_path(), &image)?;
    drive_account(&image, true)?;
    drive_account(&image, false)?;
    println!("account test passed");
    Ok(())
}

fn drive_account(disk: &std::path::Path, first: bool) -> Result<()> {
    let serial_port = 18773u16;
    let monitor_port = 18774u16;
    let mut child = Command::new("qemu-system-x86_64")
        .args([
            "-machine", "pc", "-accel", "tcg", "-cpu", "qemu64", "-smp", "1", "-m", "256M",
            "-display", "none", "-no-reboot",
        ])
        .arg("-drive")
        .arg(format!("format=raw,if=ide,index=0,file={}", image_path().display()))
        .arg("-drive")
        .arg(format!("format=raw,if=ide,index=1,cache=writethrough,file={}", disk.display()))
        .arg("-serial")
        .arg(format!("tcp:127.0.0.1:{serial_port},server"))
        .arg("-monitor")
        .arg(format!("tcp:127.0.0.1:{monitor_port},server,nowait"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("account qemu")?;
    let result = (|| {
        let serial = connect_retry(("127.0.0.1", serial_port), Duration::from_secs(20))?;
        let mut writer = serial.try_clone()?;
        let captured = Arc::new(Mutex::new(String::new()));
        let reader = Arc::clone(&captured);
        thread::spawn(move || {
            let mut serial = serial;
            let mut buf = [0u8; 256];
            loop {
                match serial.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => reader.lock().expect("serial").push_str(&String::from_utf8_lossy(&buf[..n])),
                }
            }
        });
        if first {
            if !wait_for(&captured, "name:", Duration::from_secs(30)) {
                bail!("first boot did not ask for a name:\n{}", captured.lock().unwrap());
            }
            writer.write_all(b"nia\rpitaya\rpitaya\r")?;
            writer.flush().ok();
        } else if !wait_for(&captured, "password:", Duration::from_secs(30)) {
            bail!("login did not ask for the password:\n{}", captured.lock().unwrap());
        } else {
            writer.write_all(b"pitaya\r")?;
            writer.flush().ok();
        }
        if !wait_for(&captured, "nia@::vch#~ ", Duration::from_secs(30)) {
            bail!("session prompt missing:\n{}", captured.lock().unwrap());
        }
        if !wait_for(&captured, "gui-ok", Duration::from_secs(10)) {
            bail!("gui missing after login:\n{}", captured.lock().unwrap());
        }
        Ok(())
    })();
    if let Ok(mut monitor) = connect_retry(("127.0.0.1", monitor_port), Duration::from_secs(3)) {
        let _ = write!(monitor, "quit\n");
        let _ = monitor.flush();
        let _ = child.wait();
    } else {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn make_fs(seed_account: bool) -> Result<()> {
    let root = workspace_root().join("target/disk-root");
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    for dir in [
        "Apps/vch",
        "Apps/hello",
        "Apps/greet",
        "Apps/badfmt",
        "AppData/vch/config",
        "AppData/vch/data",
        "AppData/vch/cache",
        "Libs",
        "Users/user",
    ] {
        fs::create_dir_all(root.join(dir))?;
    }
    let bins = workspace_root().join("target/user-build/x86_64-unknown-none/debug");
    pack(&bins.join("hello"), &root.join("Apps/hello/hello.vlr"))?;
    pack(&bins.join("greet"), &root.join("Apps/greet/greet.vlr"))?;
    pack(&bins.join("vch-bin"), &root.join("Apps/vch/vch.vlr"))?;
    fs::create_dir_all(root.join("Apps/keys"))?;
    pack(&bins.join("keys"), &root.join("Apps/keys/keys.vlr"))?;
    fs::create_dir_all(root.join("Users/user/stage"))?;
    fs::copy(
        root.join("Apps/hello/hello.vlr"),
        root.join("Users/user/stage/widget.vlr"),
    )?;
    fs::write(root.join("Apps/badfmt/badfmt.vlr"), b"not-a-vlr")?;
    fs::write(root.join("Users/user/motd"), b"vlr-motd\n")?;
    fs::write(root.join("Libs/.keep"), b"")?;
    fs::write(root.join("AppData/vch/config/.keep"), b"")?;
    fs::write(root.join("AppData/vch/data/.keep"), b"")?;
    fs::write(root.join("AppData/vch/cache/.keep"), b"")?;
    if seed_account {
        fs::create_dir_all(root.join("System"))?;
        fs::write(
            root.join("System/account"),
            b"name=user\nsalt=766c720000000000\nhash=-\n",
        )?;
    }
    let image = disk_path();
    if image.exists() {
        fs::remove_file(&image)?;
    }
    fs::write(&image, vec![0u8; 64 * 1024 * 1024])?;
    let status = Command::new("mkfs.btrfs")
        .args([
            "-q",
            "-f",
            "-m",
            "single",
            "-d",
            "single",
            "-r",
            root.to_str().context("root path")?,
            "--shrink",
            image.to_str().context("image path")?,
        ])
        .status()
        .context("mkfs.btrfs")?;
    if !status.success() {
        bail!("mkfs.btrfs failed");
    }
    Ok(())
}

fn pack(elf_path: &std::path::Path, dest: &std::path::Path) -> Result<()> {
    let elf = fs::read(elf_path).with_context(|| format!("read {}", elf_path.display()))?;
    if elf.len() < 64 || &elf[0..4] != b"\x7fELF" {
        bail!("{} is not an ELF", elf_path.display());
    }
    let entry = u64::from_le_bytes(elf[24..32].try_into().unwrap());
    let phoff = u64::from_le_bytes(elf[32..40].try_into().unwrap()) as usize;
    let phentsize = u16::from_le_bytes(elf[54..56].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(elf[56..58].try_into().unwrap()) as usize;
    let mut loads = Vec::new();
    for index in 0..phnum {
        let at = phoff + index * phentsize;
        let kind = u32::from_le_bytes(elf[at..at + 4].try_into().unwrap());
        if kind != 1 {
            continue;
        }
        let flags = u32::from_le_bytes(elf[at + 4..at + 8].try_into().unwrap());
        let offset = u64::from_le_bytes(elf[at + 8..at + 16].try_into().unwrap());
        let vaddr = u64::from_le_bytes(elf[at + 16..at + 24].try_into().unwrap());
        let filesz = u64::from_le_bytes(elf[at + 32..at + 40].try_into().unwrap());
        let memsz = u64::from_le_bytes(elf[at + 40..at + 48].try_into().unwrap());
        loads.push((flags, offset, vaddr, filesz, memsz));
    }
    if loads.is_empty() {
        bail!("{} has no loadable segment", elf_path.display());
    }
    let image_base = loads.iter().map(|load| load.2).min().unwrap();
    let entry_rva = entry.wrapping_sub(image_base);
    let mut section_table = Vec::new();
    let mut payload = Vec::new();
    let data_start = 32 + loads.len() * 56;
    for (index, (flags, offset, vaddr, filesz, memsz)) in loads.iter().copied().enumerate() {
        let mut section = [0u8; 56];
        let name = if flags & 1 != 0 { b"text" } else { b"data" };
        section[..name.len()].copy_from_slice(name);
        if index > 0 && name == b"text" {
            section[..4].copy_from_slice(b"segm");
        }
        let file_off = (data_start + payload.len()) as u64;
        section[16..24].copy_from_slice(&file_off.to_le_bytes());
        section[24..32].copy_from_slice(&filesz.to_le_bytes());
        section[32..40].copy_from_slice(&vaddr.to_le_bytes());
        section[40..48].copy_from_slice(&memsz.to_le_bytes());
        let mut vlr_flags = 1u32;
        if flags & 2 != 0 {
            vlr_flags |= 2;
        }
        if flags & 1 != 0 {
            vlr_flags |= 4;
        }
        section[48..52].copy_from_slice(&vlr_flags.to_le_bytes());
        section_table.extend_from_slice(&section);
        let start = offset as usize;
        let end = start + filesz as usize;
        if end > elf.len() {
            bail!("segment runs past the ELF");
        }
        payload.extend_from_slice(&elf[start..end]);
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"\x7fVLR");
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&(loads.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&entry_rva.to_le_bytes());
    out.extend_from_slice(&image_base.to_le_bytes());
    out.extend_from_slice(&section_table);
    out.extend_from_slice(&payload);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(dest, out)?;
    Ok(())
}

fn connect_retry(addr: (&str, u16), timeout: Duration) -> Result<TcpStream> {
    let start = Instant::now();
    let mut last = None;
    while start.elapsed() < timeout {
        match TcpStream::connect(addr) {
            Ok(stream) => {
                stream.set_nodelay(true).ok();
                return Ok(stream);
            }
            Err(error) => {
                last = Some(error);
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    bail!("connect {}:{} failed: {last:?}", addr.0, addr.1);
}

fn wait_for(captured: &Mutex<String>, needle: &str, timeout: Duration) -> bool {
    wait_count(captured, needle, 1, timeout)
}

fn wait_count(captured: &Mutex<String>, needle: &str, times: usize, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let text = captured.lock().expect("serial buffer");
        if text.matches(needle).count() >= times {
            return true;
        }
        drop(text);
        thread::sleep(Duration::from_millis(20));
    }
    false
}
