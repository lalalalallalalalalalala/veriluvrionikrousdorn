//! Черновик формата `.vlr` и правила, по которым будущий загрузчик найдёт
//! приложение. Исполнение образа не реализовано: [`load`] узнаёт магию и
//! останавливается.
//!
//! Версия формата `0` значит «черновик, раскладка ещё может измениться».

#![cfg_attr(not(test), no_std)]

pub const MAGIC: [u8; 4] = [0x7F, b'V', b'L', b'R'];
pub const FORMAT_VERSION: u16 = 0;
pub const HEADER_LEN: usize = 32;

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
    NotImplemented,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// Узнать свой образ и отказаться его отображать: загрузчика пользователей ещё нет.
pub fn load(image: &[u8]) -> Result<(), LoadError> {
    parse_header(image).map_err(|_| LoadError::BadImage)?;
    Err(LoadError::NotImplemented)
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
    fn load_recognizes_magic_but_does_not_run() {
        assert_eq!(load(&sample()), Err(LoadError::NotImplemented));
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
