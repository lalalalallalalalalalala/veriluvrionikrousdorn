//! Формат `.vlr` версии 1 и загрузчик в кольцо 3.
//!
//! Образ: заголовок 32 байта, затем таблица секций по 56 байт, затем байты
//! секций. Точка входа — `image_base + entry_rva`. Список библиотек может быть
//! пустым: `vch` собран статически с `vlr-libc`.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

pub const MAGIC: [u8; 4] = [0x7F, b'V', b'L', b'R'];
pub const FORMAT_VERSION: u16 = 1;
pub const HEADER_LEN: usize = 32;
pub const SECTION_LEN: usize = 56;

pub const KIND_EXECUTABLE: u16 = 1;
pub const KIND_LIBRARY: u16 = 2;

pub const SECTION_READ: u32 = 1;
pub const SECTION_WRITE: u32 = 2;
pub const SECTION_EXEC: u32 = 4;

/// Имя-заглушка, пока нет учёта пользователей.
pub const PLACEHOLDER_USER: &str = "user";
pub const HOME_PATH: &str = "/Users/user";
pub const HOME_DISPLAY: &str = "~";

pub const APPS_DIR: &str = "/Apps";
pub const APP_DATA_DIR: &str = "/AppData";
pub const LIBS_DIR: &str = "/Libs";
pub const USERS_DIR: &str = "/Users";
pub const SYSTEM_DIR: &str = "/System";

pub const VCH_EXECUTABLE: &str = "/Apps/vch/vch.vlr";
pub const VCH_DATA_DIR: &str = "/AppData/vch";
pub const VCH_PRIVATE_DIR: &str = "/Apps/vch";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatError {
    TooSmall,
    BadMagic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadError {
    BadImage,
    NotFound,
    Unsupported,
    Memory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: [u8; 16],
    pub file_offset: u64,
    pub file_size: u64,
    pub virt_addr: u64,
    pub virt_size: u64,
    pub flags: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VlrHeader {
    pub magic: [u8; 4],
    pub format_version: u16,
    pub kind: u16,
    pub flags: u16,
    pub header_size: u16,
    pub section_count: u16,
    pub import_count: u16,
    pub entry_rva: u64,
    pub image_base: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppPaths {
    pub executable: &'static str,
    pub data_dir: &'static str,
    pub private_dir: &'static str,
    pub shared_libs: &'static str,
    pub home: &'static str,
}

pub fn vch_app() -> AppPaths {
    AppPaths {
        executable: VCH_EXECUTABLE,
        data_dir: VCH_DATA_DIR,
        private_dir: VCH_PRIVATE_DIR,
        shared_libs: LIBS_DIR,
        home: HOME_PATH,
    }
}

pub fn parse_header(image: &[u8]) -> Result<VlrHeader, FormatError> {
    if image.len() < HEADER_LEN {
        return Err(FormatError::TooSmall);
    }
    if image[0..4] != MAGIC {
        return Err(FormatError::BadMagic);
    }
    Ok(VlrHeader {
        magic: [image[0], image[1], image[2], image[3]],
        format_version: u16::from_le_bytes([image[4], image[5]]),
        kind: u16::from_le_bytes([image[6], image[7]]),
        flags: u16::from_le_bytes([image[8], image[9]]),
        header_size: u16::from_le_bytes([image[10], image[11]]),
        section_count: u16::from_le_bytes([image[12], image[13]]),
        import_count: u16::from_le_bytes([image[14], image[15]]),
        entry_rva: u64::from_le_bytes(image[16..24].try_into().unwrap()),
        image_base: u64::from_le_bytes(image[24..32].try_into().unwrap()),
    })
}

pub fn parse(image: &[u8]) -> Result<(VlrHeader, Vec<Section>), FormatError> {
    let header = parse_header(image)?;
    if header.format_version > FORMAT_VERSION || header.header_size < HEADER_LEN as u16 {
        return Err(FormatError::BadMagic);
    }
    let mut sections = Vec::new();
    let mut at = header.header_size as usize;
    for _ in 0..header.section_count {
        if at + SECTION_LEN > image.len() {
            return Err(FormatError::TooSmall);
        }
        let raw = &image[at..at + SECTION_LEN];
        let mut name = [0u8; 16];
        name.copy_from_slice(&raw[0..16]);
        sections.push(Section {
            name,
            file_offset: u64::from_le_bytes(raw[16..24].try_into().unwrap()),
            file_size: u64::from_le_bytes(raw[24..32].try_into().unwrap()),
            virt_addr: u64::from_le_bytes(raw[32..40].try_into().unwrap()),
            virt_size: u64::from_le_bytes(raw[40..48].try_into().unwrap()),
            flags: u32::from_le_bytes(raw[48..52].try_into().unwrap()),
        });
        at += SECTION_LEN;
    }
    Ok((header, sections))
}

pub fn load(image: &[u8]) -> Result<(), LoadError> {
    parse(image).map_err(|_| LoadError::BadImage)?;
    Ok(())
}

pub fn exec_path(path: &str) -> Result<u64, LoadError> {
    let bytes = vlr_vfs::read_file(path).map_err(|_| LoadError::NotFound)?;
    exec_bytes(file_stem(path), &bytes)
}

pub fn exec_bytes(name: &str, image: &[u8]) -> Result<u64, LoadError> {
    let (header, sections) = parse(image).map_err(|_| LoadError::BadImage)?;
    if header.kind != KIND_EXECUTABLE || header.import_count != 0 {
        return Err(LoadError::Unsupported);
    }
    let cr3 = vlr_memory::create_user_space().map_err(|_| LoadError::Memory)?;
    map_sections(cr3, image, &sections)?;
    let stack_base = 0x700000u64;
    let stack_pages = 16u64;
    for page in 0..stack_pages {
        let frame = vlr_memory::alloc_frame().ok_or(LoadError::Memory)?;
        if let Some(virt) = vlr_memory::phys_to_virt(frame) {
            unsafe { core::ptr::write_bytes(virt as *mut u8, 0, 4096) };
        }
        vlr_memory::map_user_page(cr3, stack_base + page * 4096, frame, true, false)
            .map_err(|_| LoadError::Memory)?;
    }
    let entry = header.image_base.wrapping_add(header.entry_rva);
    let stack_top = stack_base + stack_pages * 4096;
    vlr_sched::spawn_user(name, enter_user_task, cr3, entry, stack_top)
        .map(|id| id.0)
        .map_err(|_| LoadError::Memory)
}

struct MappedPage {
    frame: u64,
    writable: bool,
    executable: bool,
}

fn map_sections(cr3: u64, image: &[u8], sections: &[Section]) -> Result<(), LoadError> {
    let mut pages: BTreeMap<u64, MappedPage> = BTreeMap::new();
    for section in sections {
        if section.virt_size == 0 {
            continue;
        }
        let start = section.virt_addr & !0xfff;
        let end = section.virt_addr.saturating_add(section.virt_size);
        let end_page = (end.saturating_add(4095)) & !0xfff;
        let mut page = start;
        while page < end_page {
            if !pages.contains_key(&page) {
                let frame = vlr_memory::alloc_frame().ok_or(LoadError::Memory)?;
                let Some(dest) = vlr_memory::phys_to_virt(frame) else {
                    return Err(LoadError::Memory);
                };
                unsafe { core::ptr::write_bytes(dest as *mut u8, 0, 4096) };
                pages.insert(
                    page,
                    MappedPage {
                        frame,
                        writable: false,
                        executable: false,
                    },
                );
            }
            let entry = pages.get_mut(&page).unwrap();
            if section.flags & SECTION_WRITE != 0 {
                entry.writable = true;
            }
            if section.flags & SECTION_EXEC != 0 {
                entry.executable = true;
            }
            page = page.saturating_add(4096);
        }
        for index in 0..section.file_size {
            let virt = section.virt_addr.wrapping_add(index);
            let page = virt & !0xfff;
            let Some(entry) = pages.get(&page) else {
                return Err(LoadError::Memory);
            };
            let file_at = section.file_offset.wrapping_add(index) as usize;
            if file_at >= image.len() {
                return Err(LoadError::BadImage);
            }
            let Some(dest) = vlr_memory::phys_to_virt(entry.frame) else {
                return Err(LoadError::Memory);
            };
            unsafe {
                *((dest + (virt & 0xfff)) as *mut u8) = image[file_at];
            }
        }
    }
    for (virt, page) in pages {
        vlr_memory::map_user_page(cr3, virt, page.frame, page.writable, page.executable)
            .map_err(|_| LoadError::Memory)?;
    }
    Ok(())
}

fn enter_user_task() {
    let (ip, sp, kstack) = vlr_sched::user_start();
    vlr_arch::disable_interrupts();
    let cr3 = vlr_sched::current_cr3();
    vlr_memory::activate_cr3(cr3);
    vlr_arch::set_kernel_stack(kstack);
    unsafe { vlr_arch::enter_user(ip, sp, kstack) };
}

fn file_stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".vlr").unwrap_or(name)
}

pub fn init() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> [u8; HEADER_LEN] {
        let mut bytes = [0u8; HEADER_LEN];
        bytes[0..4].copy_from_slice(&MAGIC);
        bytes[4..6].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes[6..8].copy_from_slice(&KIND_EXECUTABLE.to_le_bytes());
        bytes[10..12].copy_from_slice(&(HEADER_LEN as u16).to_le_bytes());
        bytes[16..24].copy_from_slice(&0x1000u64.to_le_bytes());
        bytes[24..32].copy_from_slice(&0x400000u64.to_le_bytes());
        bytes
    }

    #[test]
    fn parses_draft_header() {
        let header = parse_header(&sample()).unwrap();
        assert_eq!(header.magic, MAGIC);
        assert_eq!(header.kind, KIND_EXECUTABLE);
        assert_eq!(header.entry_rva, 0x1000);
        assert_eq!(header.image_base, 0x400000);
    }

    #[test]
    fn rejects_bad_magic_and_short_input() {
        assert_eq!(parse_header(b"ELF!"), Err(FormatError::TooSmall));
        let mut bytes = sample();
        bytes[0] = 0;
        assert_eq!(parse_header(&bytes), Err(FormatError::BadMagic));
    }

    #[test]
    fn load_accepts_a_header_and_rejects_garbage() {
        assert!(load(&sample()).is_ok());
        assert_eq!(load(b"nope"), Err(LoadError::BadImage));
    }

    #[test]
    fn vch_paths_match_the_layout() {
        let app = vch_app();
        assert_eq!(app.executable, "/Apps/vch/vch.vlr");
        assert_eq!(app.data_dir, "/AppData/vch");
        assert_eq!(app.shared_libs, "/Libs");
        assert_eq!(app.home, "/Users/user");
        assert_eq!(PLACEHOLDER_USER, "user");
    }
}
