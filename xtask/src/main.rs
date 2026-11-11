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
        "build" => {
            build_kernel(false)?;
            make_image()?;
            println!("image: {}", image_path().display());
        }
        "run" => {
            build_kernel(false)?;
            make_image()?;
            run_qemu(gui)?;
        }
        "test" => {
            host_tests()?;
            boot_test()?;
            shell_test()?;
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
        .arg(format!("format=raw,if=ide,file={}", image_path().display()))
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
    command.args(["test", "-p", "vch", "-p", "vlr-loader", "-p", "vlr-memory"]);
    let status = command.status().context("failed to spawn cargo test")?;
    if !status.success() {
        bail!("host tests failed");
    }
    Ok(())
}

fn boot_test() -> Result<()> {
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
        .arg("60")
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
        .arg(format!("format=raw,if=ide,file={}", image_path().display()))
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
        "VLR kernel 0.2.0",
        "timer-ok",
        "offset-map-ok",
        "heap-large-ok",
        "heap-small-ok",
        "heap-ok",
        "page-map-ok",
        "page-identity-ok",
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

fn shell_test() -> Result<()> {
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
    if !wait_for(&captured, "user@::vch#~ pwd", Duration::from_secs(5)) {
        bail!(
            "keyboard echo of pwd missing:\n{}",
            captured.lock().unwrap()
        );
    }
    if !wait_for(&captured, "/Users/user (~)", Duration::from_secs(5)) {
        bail!("pwd output missing:\n{}", captured.lock().unwrap());
    }
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
    let start = Instant::now();
    while start.elapsed() < timeout {
        if captured.lock().expect("serial buffer").contains(needle) {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}
