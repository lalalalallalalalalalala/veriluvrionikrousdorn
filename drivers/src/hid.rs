//! Разбор дескриптора HID-отчёта. Не только boot protocol.

#[derive(Clone, Copy)]
pub struct Field {
    pub page: u16,
    pub usage: u16,
    pub usage_max: u16,
    pub min: i32,
    pub max: i32,
    pub size: u8,
    pub count: u8,
    pub bit: u16,
    pub relative: bool,
    pub array: bool,
    pub constant: bool,
}

#[derive(Clone, Copy)]
pub struct Map {
    pub fields: [Field; 16],
    pub n: u8,
    pub report_id: bool,
    /// 1 клавиатура, 2 указатель, 3 перо/планшет, 4 тачпад, 5 тачскрин.
    pub kind: u8,
}

impl Map {
    pub const fn empty() -> Self {
        const FIELD: Field = Field {
            page: 0,
            usage: 0,
            usage_max: 0,
            min: 0,
            max: 0,
            size: 0,
            count: 0,
            bit: 0,
            relative: false,
            array: false,
            constant: false,
        };
        Self {
            fields: [FIELD; 16],
            n: 0,
            report_id: false,
            kind: 0,
        }
    }
}

pub fn parse(desc: &[u8]) -> Map {
    let mut map = Map::empty();
    let mut page = 0u16;
    let mut usage = 0u16;
    let mut usage_max = 0u16;
    let mut min = 0i32;
    let mut max = 0i32;
    let mut size = 0u8;
    let mut count = 0u8;
    let mut bit = 0u16;
    let mut i = 0usize;
    while i < desc.len() {
        let prefix = desc[i];
        if prefix == 0xFE {
            break;
        }
        let size_code = prefix & 0x03;
        let nbytes = if size_code == 3 { 4 } else { size_code as usize };
        if i + 1 + nbytes > desc.len() {
            break;
        }
        let mut raw = 0u32;
        for b in 0..nbytes {
            raw |= (desc[i + 1 + b] as u32) << (8 * b);
        }
        let signed = match nbytes {
            1 => raw as i8 as i32,
            2 => raw as i16 as i32,
            _ => raw as i32,
        };
        let kind = (prefix >> 2) & 0x03;
        let tag = (prefix >> 4) & 0x0F;
        i += 1 + nbytes;
        match (kind, tag) {
            (1, 0) => page = raw as u16,
            (1, 1) if page == 0x0D => {
                map.kind = match usage {
                    0x02 | 0x20 => 3,
                    0x05 => 4,
                    0x04 => 5,
                    _ => map.kind,
                };
            }
            (2, 0) | (2, 1) => {
                let u = raw as u16;
                if usage == 0 {
                    usage = u;
                }
                usage_max = usage_max.max(u);
            }
            (2, 2) => usage_max = raw as u16,
            (1, 5) => min = signed,
            (1, 9) => max = signed,
            (1, 7) => size = raw as u8,
            (1, 8) => count = raw as u8,
            (1, 4) => map.report_id = true,
            (0, 8) => {
                if map.n < 16 && size > 0 && count > 0 {
                    map.fields[map.n as usize] = Field {
                        page,
                        usage,
                        usage_max: usage_max.max(usage),
                        min,
                        max,
                        size,
                        count,
                        bit,
                        relative: raw & 0x04 != 0,
                        array: raw & 0x02 == 0,
                        constant: raw & 1 != 0,
                    };
                    map.n += 1;
                }
                bit = bit.saturating_add(u16::from(size) * u16::from(count));
                usage = 0;
                usage_max = 0;
            }
            _ => {}
        }
    }
    map
}

pub fn field_value(data: &[u8], bit: u16, size: u8) -> u32 {
    if size == 0 || size > 32 {
        return 0;
    }
    let mut value = 0u32;
    for i in 0..size {
        let at = bit as usize + i as usize;
        let byte = data.get(at / 8).copied().unwrap_or(0);
        if byte & (1 << (at % 8)) != 0 {
            value |= 1 << i;
        }
    }
    value
}

pub fn signed_value(raw: u32, size: u8, min: i32) -> i32 {
    if min < 0 && size > 0 && size < 32 && raw & (1 << (size - 1)) != 0 {
        (raw | (!0u32 << size)) as i32
    } else {
        raw as i32
    }
}
