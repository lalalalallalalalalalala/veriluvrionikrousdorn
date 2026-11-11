//! Битовая карта физических кадров по 4 КиБ.
//!
//! Кадр свободен, только если он попал в usable-регион карты загрузчика.
//! Кадр 0 никогда не выдаётся. Память самой карты — статический буфер ядра
//! либо буфер, который передал тест на хосте.

use core::ptr;

use crate::RegionRecord;

pub const PAGE_SIZE: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreeError {
    NotAllocated,
    NotUsable,
    Misaligned,
    OutOfRange,
}

pub struct BitmapFrameAlloc {
    bitmap: *mut u8,
    bitmap_len: usize,
    regions: [RegionRecord; 64],
    region_len: usize,
    total: usize,
    used: usize,
    next_hint: usize,
}

unsafe impl Send for BitmapFrameAlloc {}

impl BitmapFrameAlloc {
    pub fn from_regions(bitmap: &mut [u8], regions: &[RegionRecord]) -> Self {
        bitmap.fill(0xFF);
        let mut alloc = Self {
            bitmap: bitmap.as_mut_ptr(),
            bitmap_len: bitmap.len(),
            regions: [RegionRecord {
                start: 0,
                end: 0,
                usable: false,
            }; 64],
            region_len: 0,
            total: 0,
            used: 0,
            next_hint: 1,
        };
        for region in regions {
            if alloc.region_len < alloc.regions.len() {
                alloc.regions[alloc.region_len] = *region;
                alloc.region_len += 1;
            }
            if !region.usable {
                continue;
            }
            let start = align_up(region.start, PAGE_SIZE);
            let end = align_down(region.end, PAGE_SIZE);
            let mut addr = start;
            while addr.saturating_add(PAGE_SIZE) <= end {
                let index = (addr / PAGE_SIZE) as usize;
                if index >= alloc.frame_slots() {
                    break;
                }
                if index != 0 && alloc.bit(index) {
                    alloc.set_bit(index, false);
                    alloc.total += 1;
                }
                addr = addr.saturating_add(PAGE_SIZE);
            }
        }
        if alloc.total > 0 {
            alloc.next_hint = 1;
        }
        alloc
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn used(&self) -> usize {
        self.used
    }

    pub fn free(&self) -> usize {
        self.total.saturating_sub(self.used)
    }

    pub fn allocate(&mut self) -> Option<u64> {
        let slots = self.frame_slots();
        if slots == 0 || self.free() == 0 {
            return None;
        }
        let start = self.next_hint.min(slots - 1);
        for step in 0..slots {
            let index = (start + step) % slots;
            if index == 0 || self.bit(index) {
                continue;
            }
            self.set_bit(index, true);
            self.used += 1;
            self.next_hint = (index + 1) % slots;
            return Some((index as u64) * PAGE_SIZE);
        }
        None
    }

    pub fn deallocate(&mut self, phys: u64) -> Result<(), FreeError> {
        if phys == 0 || phys % PAGE_SIZE != 0 {
            return Err(FreeError::Misaligned);
        }
        let index = (phys / PAGE_SIZE) as usize;
        if index >= self.frame_slots() {
            return Err(FreeError::OutOfRange);
        }
        if !self.contains_usable(phys) {
            return Err(FreeError::NotUsable);
        }
        if !self.bit(index) {
            return Err(FreeError::NotAllocated);
        }
        self.set_bit(index, false);
        self.used = self.used.saturating_sub(1);
        self.next_hint = index;
        Ok(())
    }

    fn frame_slots(&self) -> usize {
        self.bitmap_len.saturating_mul(8)
    }

    fn contains_usable(&self, phys: u64) -> bool {
        self.regions[..self.region_len]
            .iter()
            .any(|region| region.usable && phys >= region.start && phys < region.end)
    }

    fn bit(&self, index: usize) -> bool {
        let byte = unsafe { ptr::read_volatile(self.bitmap.add(index / 8)) };
        byte & (1 << (index % 8)) != 0
    }

    fn set_bit(&mut self, index: usize, value: bool) {
        unsafe {
            let cell = self.bitmap.add(index / 8);
            let mut byte = ptr::read_volatile(cell);
            let mask = 1 << (index % 8);
            if value {
                byte |= mask;
            } else {
                byte &= !mask;
            }
            ptr::write_volatile(cell, byte);
        }
    }
}

pub fn align_up(value: u64, align: u64) -> u64 {
    value.div_ceil(align) * align
}

pub fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(start: u64, end: u64, usable: bool) -> RegionRecord {
        RegionRecord { start, end, usable }
    }

    #[test]
    fn allocates_only_usable_aligned_frames_and_reuses_them() {
        let mut bitmap = [0u8; 32];
        let regions = [
            region(0, 0x2000, false),
            region(0x1001, 0x5000, true),
            region(0x5000, 0x6000, false),
        ];
        let mut alloc = BitmapFrameAlloc::from_regions(&mut bitmap, &regions);
        // 0x1001 выравнивается до 0x2000, конец 0x5000: кадры 0x2000, 0x3000, 0x4000.
        assert_eq!(alloc.total(), 3);
        assert_eq!(alloc.free(), 3);

        let mut got = [
            alloc.allocate().unwrap(),
            alloc.allocate().unwrap(),
            alloc.allocate().unwrap(),
        ];
        got.sort_unstable();
        assert_eq!(got, [0x2000, 0x3000, 0x4000]);
        assert!(alloc.allocate().is_none());
        assert_eq!(alloc.used(), 3);

        alloc.deallocate(0x3000).unwrap();
        assert_eq!(alloc.free(), 1);
        assert_eq!(alloc.allocate(), Some(0x3000));
        alloc.deallocate(0x3000).unwrap();
        assert_eq!(alloc.deallocate(0x3000), Err(FreeError::NotAllocated));
        assert_eq!(alloc.deallocate(0x1000), Err(FreeError::NotUsable));
        assert_eq!(alloc.deallocate(0x2001), Err(FreeError::Misaligned));
    }
}
