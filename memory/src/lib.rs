//! Память. В этой вехе модуль только запоминает карту, которую отдал загрузчик.
//!
//! Построение таблиц страниц и куча — следующие шаги. Трейт [`FrameAllocator`]
//! фиксирует контракт, чтобы аллокатор кадров появился здесь, а не в драйверах.

#![no_std]

use spin::Once;

const MAX_REGIONS: usize = 64;

static STATE: Once<MemoryState> = Once::new();

#[derive(Clone, Copy, Debug)]
pub struct RegionRecord {
    pub start: u64,
    pub end: u64,
    pub usable: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct PhysFrame {
    pub start: u64,
}

pub trait FrameAllocator {
    fn allocate(&mut self) -> Option<PhysFrame>;
    fn deallocate(&mut self, frame: PhysFrame);
}

pub struct MemoryState {
    regions: [RegionRecord; MAX_REGIONS],
    region_len: usize,
    truncated: bool,
    usable_bytes: u64,
    physical_memory_offset: Option<u64>,
}

pub fn init(regions: &[RegionRecord], physical_memory_offset: Option<u64>) -> &'static MemoryState {
    STATE.call_once(|| {
        let mut state = MemoryState {
            regions: [RegionRecord {
                start: 0,
                end: 0,
                usable: false,
            }; MAX_REGIONS],
            region_len: 0,
            truncated: false,
            usable_bytes: 0,
            physical_memory_offset,
        };
        for region in regions {
            let size = region.end.saturating_sub(region.start);
            if region.usable {
                state.usable_bytes = state.usable_bytes.saturating_add(size);
            }
            if state.region_len < MAX_REGIONS {
                state.regions[state.region_len] = *region;
                state.region_len += 1;
            } else {
                state.truncated = true;
            }
        }
        state
    })
}

impl MemoryState {
    pub fn usable_bytes(&self) -> u64 {
        self.usable_bytes
    }

    pub fn region_count(&self) -> usize {
        self.region_len
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn physical_memory_offset(&self) -> Option<u64> {
        self.physical_memory_offset
    }

    pub fn regions(&self) -> &[RegionRecord] {
        &self.regions[..self.region_len]
    }
}
