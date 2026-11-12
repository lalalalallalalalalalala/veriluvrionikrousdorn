//! Ядро Veriluvrionikrousdorn. Здесь только сборка подсистем: каждая из них
//! живёт в своём крейте.

#![no_std]
#![no_main]

extern crate alloc;

use bootloader_api::config::Mapping;
use bootloader_api::info::{MemoryRegionKind, PixelFormat};
use bootloader_api::{entry_point, BootInfo, BootloaderConfig};
use vlr_drivers::display::FramebufferConfig;
use vlr_memory::RegionRecord;

mod userland;

#[cfg(feature = "shell")]
mod session;

const OS_NAME: &str = "Veriluvrionikrousdorn";
const OS_VERSION: &str = env!("CARGO_PKG_VERSION");

static USABLE_MEMORY: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.kernel_stack_size = 128 * 1024;
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config
};

#[global_allocator]
static GLOBAL_ALLOCATOR: vlr_memory::KernelAllocator = vlr_memory::KernelAllocator::new();

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    vlr_arch::disable_interrupts();
    vlr_drivers::serial::init();
    vlr_drivers::serial::write_str("diag: serial ok\n");

    let mut stored = [RegionRecord {
        start: 0,
        end: 0,
        usable: false,
    }; 64];
    let mut count = 0usize;
    for region in boot_info.memory_regions.iter() {
        if count == stored.len() {
            break;
        }
        stored[count] = RegionRecord {
            start: region.start,
            end: region.end,
            usable: region.kind == MemoryRegionKind::Usable,
        };
        count += 1;
    }
    let physical_offset = boot_info.physical_memory_offset.into_option();
    let memory = vlr_memory::init(&stored[..count], physical_offset);

    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
        let info = framebuffer.info();
        let buffer = framebuffer.buffer_mut();
        vlr_drivers::display::init(FramebufferConfig {
            ptr: buffer.as_mut_ptr(),
            len: buffer.len(),
            width: info.width,
            height: info.height,
            stride: info.stride,
            bytes_per_pixel: info.bytes_per_pixel,
            bgr: matches!(info.pixel_format, PixelFormat::Bgr),
        });
        vlr_drivers::serial::write_str("diag: display ok\n");
    } else {
        vlr_drivers::serial::write_str("diag: no framebuffer, serial only\n");
    }

    vlr_arch::set_exception_logger(log_exception);
    vlr_arch::init();
    vlr_drivers::serial::write_str("diag: arch ok\n");

    let offset = physical_offset.expect("bootloader did not map physical memory");
    unsafe { vlr_memory::init_paging(offset) };
    if let Err(err) = vlr_memory::init_heap() {
        vlr_drivers::serial::write_str("diag: heap failed\n");
        let _ = err;
        vlr_arch::debug_exit(false);
    }
    vlr_drivers::serial::write_str("diag: heap ok\n");

    vlr_sched::init();
    vlr_arch::set_user_fault_hook(on_user_fault);
    vlr_drivers::timer::init();
    vlr_drivers::keyboard::init();
    vlr_drivers::serial::enable_rx_irq();
    vlr_syscall::init();
    #[cfg(feature = "shell")]
    vlr_syscall::set_demo_spawner(spawn_named);
    vlr_loader::init();
    mount_disk(boot_info);
    vlr_drivers::serial::write_str("diag: devices armed\n");

    USABLE_MEMORY.store(memory.usable_bytes(), core::sync::atomic::Ordering::Relaxed);
    print_banner(memory.usable_bytes());

    vlr_arch::enable_interrupts();

    #[cfg(all(feature = "shell", not(feature = "boot-test")))]
    bring_up_desktop();

    #[cfg(feature = "boot-test")]
    boot_test();

    #[cfg(feature = "shell")]
    vlr_sched::idle();
}

fn print_banner(usable_bytes: u64) {
    vlr_console::set_color(0x00E0B15A);
    vlr_console::println!("{OS_NAME}");
    vlr_console::set_color(0x00E7E2D6);
    vlr_console::println!("VLR kernel {OS_VERSION}");
    vlr_console::println!("x86-64 btrfs");
    let mib = usable_bytes / (1024 * 1024);
    vlr_console::println!("usable memory: {mib} MiB");
    vlr_console::println!();
}

#[cfg(feature = "boot-test")]
fn boot_test() -> ! {
    while vlr_drivers::timer::ticks() < 5 {
        vlr_arch::hlt();
    }
    let ticks = vlr_drivers::timer::ticks();
    vlr_console::println!("timer-ok ticks={ticks}");

    let stats = vlr_memory::stats();
    vlr_console::println!(
        "frames total={} used={} free={}",
        stats.frames_total,
        stats.frames_used,
        stats.frames_free
    );
    if stats.frames_total == 0 || stats.frames_free == 0 {
        fail("frames-bad");
    }
    let offset = stats.phys_offset.expect("offset");
    if vlr_memory::translate(offset) != Some(0) {
        fail("offset-map-bad");
    }
    vlr_console::println!("offset-map-ok");

    heap_checks();
    paging_checks();
    sched_checks();
    user_checks();
    disk_checks();

    const FAULT_ADDR: u64 = 0xFFFF_8200_0000_0000;
    vlr_arch::set_page_fault_hook(on_test_fault);
    unsafe {
        core::ptr::read_volatile(FAULT_ADDR as *const u8);
    }
    fail("page-fault-missing");
}

#[cfg(feature = "boot-test")]
fn heap_checks() {
    let mut big = alloc::vec![0u8; 1024 * 1024];
    for (index, byte) in big.iter_mut().enumerate() {
        *byte = (index & 0xFF) as u8;
    }
    let intact = big
        .iter()
        .enumerate()
        .all(|(index, byte)| *byte == (index & 0xFF) as u8);
    if !intact || big.len() != 1024 * 1024 {
        fail("heap-large-bad");
    }
    vlr_console::println!("heap-large-ok bytes={}", big.len());
    drop(big);

    let mut small = alloc::vec::Vec::new();
    for index in 0..1000u32 {
        small.push(alloc::boxed::Box::new(index));
    }
    let intact = small
        .iter()
        .enumerate()
        .all(|(index, value)| **value == index as u32);
    if !intact || small.len() != 1000 {
        fail("heap-small-bad");
    }
    vlr_console::println!("heap-small-ok count={}", small.len());
    drop(small);

    let mut map = alloc::collections::BTreeMap::new();
    map.insert(1u64, alloc::string::String::from("vlr"));
    if map.get(&1).map(alloc::string::String::as_str) != Some("vlr") {
        fail("heap-map-bad");
    }
    vlr_console::println!("heap-ok");
}

#[cfg(feature = "boot-test")]
fn paging_checks() {
    let frame = vlr_memory::alloc_frame().unwrap_or_else(|| fail("no-frame"));
    let virt = 0xFFFF_8200_0000_0000u64;
    if vlr_memory::map_4k(virt, frame, true).is_err() {
        fail("map-bad");
    }
    unsafe {
        (virt as *mut u64).write_volatile(0x1122_3344_5566_7788);
    }
    let saw = unsafe { core::ptr::read_volatile(virt as *const u64) };
    let unmapped = vlr_memory::unmap_4k(virt).ok();
    if vlr_memory::free_frame(frame).is_ok() {
        fail("double-free");
    }
    if saw != 0x1122_3344_5566_7788 || unmapped != Some(frame) {
        fail("page-map-bad");
    }
    vlr_console::println!("page-map-ok");

    let identity = vlr_memory::alloc_frame().unwrap_or_else(|| fail("no-identity-frame"));
    if vlr_memory::translate(identity).is_some() {
        let _ = vlr_memory::free_frame(identity);
        fail("identity-slot-busy");
    }
    if vlr_memory::map_identity(identity).is_err() {
        fail("identity-map-bad");
    }
    unsafe {
        (identity as *mut u64).write_volatile(0x00C0_FFEE);
    }
    let via_offset = vlr_memory::phys_to_virt(identity).unwrap_or_else(|| fail("no-offset"));
    let saw = unsafe { core::ptr::read_volatile(via_offset as *const u64) };
    let unmapped = vlr_memory::unmap_4k(identity).ok();
    if vlr_memory::free_frame(identity).is_ok() {
        fail("identity-double-free");
    }
    if saw != 0x00C0_FFEE || unmapped != Some(identity) {
        fail("page-identity-bad");
    }
    vlr_console::println!("page-identity-ok");
}

#[cfg(feature = "boot-test")]
static ALPHA: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "boot-test")]
static BETA: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "boot-test")]
static SLEEPER_DONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

#[cfg(feature = "boot-test")]
fn alpha_worker() {
    loop {
        ALPHA.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        core::hint::spin_loop();
    }
}

#[cfg(feature = "boot-test")]
fn beta_worker() {
    loop {
        BETA.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        core::hint::spin_loop();
    }
}

#[cfg(feature = "boot-test")]
fn sleeper_worker() {
    vlr_sched::sleep(20);
    SLEEPER_DONE.store(true, core::sync::atomic::Ordering::Release);
    vlr_sched::exit();
}

#[cfg(feature = "boot-test")]
fn quitter_worker() {
    vlr_sched::exit();
}

#[cfg(feature = "boot-test")]
fn sched_checks() {
    if vlr_sched::spawn("alpha", alpha_worker).is_err()
        || vlr_sched::spawn("beta", beta_worker).is_err()
        || vlr_sched::spawn("sleeper", sleeper_worker).is_err()
        || vlr_sched::spawn("quitter", quitter_worker).is_err()
    {
        fail("spawn-bad");
    }
    let start = vlr_drivers::timer::ticks();
    loop {
        let alpha = ALPHA.load(core::sync::atomic::Ordering::Relaxed);
        let beta = BETA.load(core::sync::atomic::Ordering::Relaxed);
        let slept = SLEEPER_DONE.load(core::sync::atomic::Ordering::Acquire);
        let reaped = !vlr_sched::exists("quitter");
        if alpha > 0 && beta > 0 && slept && reaped {
            vlr_console::println!("sched-ok alpha={alpha} beta={beta}");
            vlr_console::println!("sleep-ok");
            vlr_console::println!("reap-ok");
            break;
        }
        if vlr_drivers::timer::ticks().saturating_sub(start) > 500 {
            vlr_console::println!(
                "sched-bad alpha={alpha} beta={beta} slept={slept} reaped={reaped}"
            );
            fail("sched-timeout");
        }
        core::hint::spin_loop();
    }
}

#[cfg(feature = "boot-test")]
fn user_checks() {
    if vlr_sched::spawn("hello", userland::launch_hello).is_err() {
        fail("user-spawn-bad");
    }
    let start = vlr_drivers::timer::ticks();
    while vlr_sched::exists("hello") {
        if vlr_drivers::timer::ticks().saturating_sub(start) > 300 {
            fail("user-hello-timeout");
        }
        core::hint::spin_loop();
    }
    vlr_console::println!("user-hello-ok");

    if vlr_sched::spawn("bad", userland::launch_bad).is_err() {
        fail("user-bad-spawn");
    }
    let start = vlr_drivers::timer::ticks();
    while vlr_sched::exists("bad") {
        if vlr_drivers::timer::ticks().saturating_sub(start) > 300 {
            fail("user-bad-timeout");
        }
        core::hint::spin_loop();
    }
    vlr_console::println!("user-bad-ok");
}

fn mount_disk(boot_info: &'static mut BootInfo) {
    if let Some(addr) = boot_info.ramdisk_addr.into_option() {
        if boot_info.ramdisk_len >= 4096 {
            vlr_drivers::ramdisk::init(addr as *mut u8, boot_info.ramdisk_len as usize);
            vlr_drivers::serial::write_str("live-ok\n");
        }
    }
    if vlr_drivers::usb_msc::claim_root() {
        match vlr_vfs::mount() {
            Ok(()) => vlr_console::println!("btrfs-ok"),
            Err(_) => vlr_console::println!("btrfs-failed"),
        }
        return;
    }
    if vlr_drivers::ramdisk::active() {
        match vlr_vfs::mount() {
            Ok(()) => vlr_console::println!("btrfs-ok"),
            Err(_) => vlr_console::println!("btrfs-failed"),
        }
        return;
    }
    match vlr_drivers::ata::init() {
        Ok(()) => match vlr_vfs::mount() {
            Ok(()) => vlr_console::println!("btrfs-ok"),
            Err(_) => vlr_console::println!("btrfs-failed"),
        },
        Err(_) => vlr_console::println!("ata-failed"),
    }
}

#[cfg(feature = "boot-test")]
fn disk_checks() {
    match vlr_vfs::read_file("/Users/user/motd") {
        Ok(bytes) if bytes == b"vlr-motd\n" => vlr_console::println!("motd-ok"),
        Ok(_) => fail("motd-bad"),
        Err(_) => fail("motd-bad"),
    }
    match vlr_vfs::list_dir("/Apps") {
        Ok(names) if names.iter().any(|name| name == "vch") && names.iter().any(|name| name == "badfmt") => {
            vlr_console::println!("apps-ok");
        }
        Ok(_) => fail("apps-missing"),
        Err(_) => fail("apps-bad"),
    }
    match vlr_loader::exec_path("/Apps/vch/vch.vlr") {
        Ok(pid) => vlr_console::println!("vch-loaded pid={pid}"),
        Err(err) => {
            vlr_console::println!("vch-load-fail {err:?}");
            fail("vch-load");
        }
    }
    vlr_sched::sleep(40);
    inject("ls /Apps\n");
    vlr_sched::sleep(80);
    inject("cat /Users/user/motd\n");
    vlr_sched::sleep(80);
    inject("/Apps/hello/hello.vlr\n");
    vlr_sched::sleep(80);
    inject("/Apps/badfmt/badfmt.vlr\n");
    vlr_sched::sleep(80);
    vlr_console::println!("disk-shell-ok");
}

#[cfg(feature = "boot-test")]
fn inject(line: &str) {
    for byte in line.bytes() {
        vlr_drivers::input::push(byte);
    }
}

fn on_user_fault(addr: u64, kind: &'static str) -> ! {
    let id = vlr_sched::current_id();
    vlr_console::println!("user fault: {kind} pid={id} addr={addr:#x}");
    vlr_sched::exit();
}

#[cfg(feature = "boot-test")]
fn on_test_fault(addr: u64) -> ! {
    vlr_console::println!("page-fault-ok addr={addr:#x}");
    vlr_console::println!("vlr-boot-ok");
    vlr_arch::debug_exit(true);
}

#[cfg(feature = "boot-test")]
fn fail(message: &str) -> ! {
    vlr_console::println!("boot-test-fail {message}");
    vlr_arch::debug_exit(false);
}

fn log_exception(message: &str) {
    vlr_drivers::serial::write_str(message);
    let _ = vlr_drivers::display::try_write_str(message);
}

#[cfg(all(feature = "shell", not(feature = "boot-test")))]
fn bring_up_desktop() {
    let name = session::establish();
    vlr_gui::set_checker(session::check_password);
    vlr_syscall::set_install_hook(vlr_gui::refresh_apps);
    vlr_drivers::mouse::init();
    if vlr_drivers::display::is_ready() {
        vlr_gui::set_user(&name);
        vlr_drivers::serial::write_str("gui-ok\n");
        let _ = vlr_sched::spawn("gui", gui_task);
    }
    start_shell();
}

#[cfg(all(feature = "shell", not(feature = "boot-test")))]
fn gui_task() {
    vlr_gui::desktop();
}

#[cfg(all(feature = "shell", not(feature = "boot-test")))]
fn start_shell() {
    let _ = vlr_sched::spawn("ticker", ticker_task);
    let _ = vlr_sched::spawn("spinner", spinner_task);
    match vlr_loader::exec_path("/Apps/vch/vch.vlr") {
        Ok(pid) => vlr_console::println!("vch-loaded pid={pid}"),
        Err(err) => {
            vlr_console::println!("vch-fallback {err:?}");
            let _ = vlr_sched::spawn("vch", vch_task);
        }
    }
}

#[cfg(feature = "shell")]
fn vch_task() {
    let name = leak_str(vlr_btrfs::session_name());
    let home = leak_str(vlr_account::home_of(name));
    let info = vch::ShellInfo {
        username: name,
        os_name: OS_NAME,
        os_version: OS_VERSION,
        usable_memory: USABLE_MEMORY.load(core::sync::atomic::Ordering::Relaxed),
        home_display: vlr_loader::HOME_DISPLAY,
        home_path: home,
    };
    let mut shell = KernelShell {
        cwd: alloc::string::String::from(home),
    };
    vch::run(&mut shell, &info);
}

#[cfg(feature = "shell")]
fn leak_str(text: alloc::string::String) -> &'static str {
    alloc::boxed::Box::leak(text.into_boxed_str())
}

#[cfg(feature = "shell")]
fn ticker_task() {
    loop {
        vlr_sched::sleep(50);
    }
}

#[cfg(feature = "shell")]
fn spinner_task() {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(feature = "shell")]
fn spawn_named(name: &str) -> Result<u64, &'static str> {
    let entry = match name {
        "ticker" => ticker_task,
        "spinner" => spinner_task,
        _ => return Err("unknown task"),
    };
    vlr_sched::spawn(name, entry)
        .map(|id| id.0)
        .map_err(|_| "spawn failed")
}

#[cfg(feature = "shell")]
struct KernelShell {
    cwd: alloc::string::String,
}

#[cfg(feature = "shell")]
impl vch::ShellIo for KernelShell {
    fn write_str(&mut self, text: &str) {
        vlr_console::write_str(text);
    }

    fn clear(&mut self) {
        vlr_console::clear();
    }

    fn backspace(&mut self) {
        vlr_console::backspace();
    }

    fn read_byte(&mut self) -> u8 {
        vlr_drivers::input::read_byte()
    }

    fn ticks(&mut self) -> u64 {
        vlr_drivers::timer::ticks()
    }

    fn memory(&mut self) -> vch::MemorySnapshot {
        let stats = vlr_memory::stats();
        vch::MemorySnapshot {
            frames_total: stats.frames_total as u64,
            frames_used: stats.frames_used as u64,
            frames_free: stats.frames_free as u64,
            heap_total: stats.heap_total as u64,
            heap_used: stats.heap_used as u64,
            heap_free: stats.heap_free as u64,
        }
    }

    fn tasks(&mut self) -> alloc::vec::Vec<vch::TaskSnapshot> {
        let mut raw = [vlr_sched::TaskInfo::empty(); vlr_sched::MAX_TASKS];
        let count = vlr_sched::snapshot(&mut raw);
        raw[..count]
            .iter()
            .map(|task| vch::TaskSnapshot {
                id: task.id,
                name: core::str::from_utf8(&task.name[..task.name_len as usize])
                    .unwrap_or("?")
                    .into(),
                mode: task.mode,
                state: task.state,
                ticks: task.ticks,
            })
            .collect()
    }

    fn spawn_task(&mut self, name: &str) -> Result<u64, &'static str> {
        spawn_named(name)
    }

    fn kill_task(&mut self, id: u64) -> Result<(), &'static str> {
        vlr_sched::kill(id).map_err(|_| "kill failed")
    }

    fn run_user(&mut self, name: &str) -> Result<u64, &'static str> {
        userland::launch_named(name)
    }

    fn current_dir(&mut self) -> alloc::string::String {
        self.cwd.clone()
    }

    fn change_dir(&mut self, path: &str) {
        self.cwd = alloc::string::String::from(path);
    }

    fn list_dir(&mut self, path: &str) -> Result<alloc::vec::Vec<alloc::string::String>, &'static str> {
        vlr_vfs::list_dir(path).map_err(vfs_err)
    }

    fn read_file(&mut self, path: &str) -> Result<alloc::vec::Vec<u8>, &'static str> {
        vlr_vfs::read_file(path).map_err(vfs_err)
    }

    fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), &'static str> {
        vlr_vfs::write_file(path, data).map_err(vfs_err)
    }

    fn create_file(&mut self, path: &str) -> Result<(), &'static str> {
        vlr_vfs::create_file(path).map_err(vfs_err)
    }

    fn make_dir(&mut self, path: &str) -> Result<(), &'static str> {
        vlr_vfs::make_dir(path).map_err(vfs_err)
    }

    fn remove_path(&mut self, path: &str) -> Result<(), &'static str> {
        vlr_vfs::remove_path(path).map_err(vfs_err)
    }

    fn rename_path(&mut self, from: &str, to: &str) -> Result<(), &'static str> {
        vlr_vfs::rename_path(from, to).map_err(vfs_err)
    }

    fn install_app(&mut self, path: &str) -> Result<(), &'static str> {
        vlr_syscall::install_file(path)
    }

    fn uninstall_app(&mut self, name: &str) -> Result<(), &'static str> {
        vlr_syscall::uninstall_name(name)
    }

    fn exec(&mut self, path: &str) -> Result<u64, &'static str> {
        match vlr_loader::exec_path(path) {
            Ok(id) => Ok(id),
            Err(vlr_loader::LoadError::BadImage | vlr_loader::LoadError::Unsupported) => {
                Err("rejected")
            }
            Err(_) => Err("not found"),
        }
    }
}

#[cfg(feature = "shell")]
fn vfs_err(err: vlr_vfs::VfsError) -> &'static str {
    match err {
        vlr_vfs::VfsError::NotFound => "not found",
        vlr_vfs::VfsError::NotDirectory => "not a directory",
        vlr_vfs::VfsError::Compressed => "compressed",
        vlr_vfs::VfsError::ReadOnly => "read only",
        vlr_vfs::VfsError::Exists => "exists",
        vlr_vfs::VfsError::NotEmpty => "not empty",
        _ => "io error",
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    vlr_drivers::serial::write_str("\nkernel panic: ");
    let mut raw = [0u8; 512];
    let used = {
        let mut writer = PanicWriter {
            buf: &mut raw,
            used: 0,
        };
        let _ = core::fmt::write(&mut writer, format_args!("{info}\n"));
        writer.used
    };
    let message = core::str::from_utf8(&raw[..used]).unwrap_or("panic\n");
    vlr_drivers::serial::write_str(message);
    let _ = vlr_drivers::display::try_write_str("\nkernel panic: ");
    let _ = vlr_drivers::display::try_write_str(message);
    vlr_arch::hlt_loop();
}

struct PanicWriter<'a> {
    buf: &'a mut [u8],
    used: usize,
}

impl core::fmt::Write for PanicWriter<'_> {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let bytes = text.as_bytes();
        let room = self.buf.len().saturating_sub(self.used);
        let count = bytes.len().min(room);
        self.buf[self.used..self.used + count].copy_from_slice(&bytes[..count]);
        self.used += count;
        Ok(())
    }
}
