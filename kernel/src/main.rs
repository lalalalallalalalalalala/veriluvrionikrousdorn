//! Ядро Veriluvrionikrousdorn. Здесь только сборка подсистем: каждая из них
//! живёт в своём крейте.

#![no_std]
#![no_main]

use bootloader_api::config::Mapping;
use bootloader_api::info::{MemoryRegionKind, PixelFormat};
use bootloader_api::{entry_point, BootInfo, BootloaderConfig};
use vlr_drivers::display::FramebufferConfig;
use vlr_memory::RegionRecord;

const OS_NAME: &str = "Veriluvrionikrousdorn";
const OS_VERSION: &str = env!("CARGO_PKG_VERSION");

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.kernel_stack_size = 128 * 1024;
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config
};

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

    vlr_drivers::timer::init();
    vlr_drivers::keyboard::init();
    vlr_drivers::serial::enable_rx_irq();
    let _scheduler = vlr_sched::init();
    vlr_syscall::init();
    let _vfs = vlr_vfs::init();
    vlr_btrfs::init();
    vlr_loader::init();
    vlr_drivers::serial::write_str("diag: devices armed\n");

    print_banner(memory.usable_bytes());
    vlr_arch::enable_interrupts();

    #[cfg(feature = "boot-test")]
    boot_test();

    #[cfg(feature = "shell")]
    {
        let info = vch::ShellInfo {
            username: vlr_loader::PLACEHOLDER_USER,
            os_name: OS_NAME,
            os_version: OS_VERSION,
            usable_memory: memory.usable_bytes(),
            home_display: vlr_loader::HOME_DISPLAY,
            home_path: vlr_loader::HOME_PATH,
        };
        let mut shell = KernelShell;
        vch::run(&mut shell, &info);
    }
}

fn print_banner(usable_bytes: u64) {
    vlr_console::set_color(0x00E0B15A);
    vlr_console::println!("{OS_NAME}");
    vlr_console::set_color(0x00E7E2D6);
    vlr_console::println!("VLR kernel {OS_VERSION}");
    vlr_console::println!("x86-64 foundation");
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
    vlr_console::println!("vlr-boot-ok");
    vlr_arch::debug_exit(true);
}

fn log_exception(message: &str) {
    vlr_drivers::serial::write_str(message);
    let _ = vlr_drivers::display::try_write_str(message);
}

#[cfg(feature = "shell")]
struct KernelShell;

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
