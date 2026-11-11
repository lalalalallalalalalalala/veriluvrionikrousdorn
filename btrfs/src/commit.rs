//! Копирующая запись Btrfs.
//!
//! Новые блоки деревьев и данных занимают свободные места. Старые блоки
//! остаются, пока суперблок не переключится на новый корень. Обрыв до этой
//! записи оставляет прежнее поколение.

use alloc::vec::Vec;

use super::{
    name_hash, read_bytes, u16_at, u32_at, u64_at, BtrfsError, Volume, SUPERBLOCK_OFFSET,
};
use vlr_drivers::block::BlockDevice;

const INLINE_MAX: usize = 2048;
const META_ITEM: u8 = 169;
const DATA_ITEM: u8 = 168;
const BLOCK_GROUP: u8 = 192;
const INODE_REF: u8 = 12;
const CSUM_ITEM: u8 = 128;
const FREE_INFO: u8 = 198;
const FREE_EXTENT: u8 = 199;
const CSUM_OBJECT: u64 = u64::MAX - 9;
const FT_FILE: u8 = 1;
const FT_DIR: u8 = 2;
const FLAG_DATA: u64 = 1;
const FLAG_TREE: u64 = 2;
const BG_DATA: u64 = 1;
const BG_META: u64 = 4;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    obj: u64,
    typ: u8,
    off: u64,
}

struct Leaf {
    header: Vec<u8>,
    items: Vec<(Key, Vec<u8>)>,
    nodesize: usize,
}

#[derive(Clone, Copy)]
struct Bg {
    start: u64,
    len: u64,
    flags: u64,
}

pub(crate) struct Tx {
    nodesize: usize,
    generation: u64,
    superblock: Vec<u8>,
    fs: Leaf,
    extent: Leaf,
    space: Leaf,
    root: Leaf,
    csum: Leaf,
    fs_addr: u64,
    extent_addr: u64,
    space_addr: u64,
    root_addr: u64,
    csum_addr: u64,
    groups: Vec<Bg>,
    free: Vec<(u64, u64)>,
    info: Vec<(u64, Vec<u8>)>,
    pub(crate) dirty: bool,
    csum_dirty: bool,
    data_delta: i64,
    /// Данные, которые нужно записать в новый экстент перед суперблоком.
    payload: Option<Payload>,
    /// Старые экстенты данных, которые освободятся только в новом поколении.
    retired: Vec<(u64, u64)>,
}

struct Payload {
    inode: u64,
    bytes: Vec<u8>,
    addr: u64,
    disk_len: u64,
}

pub fn may_write(path: &str) -> bool {
    let path = path.trim_end_matches('/');
    path == "/Users/user"
        || path.starts_with("/Users/user/")
        || (path.starts_with("/AppData/") && path != "/AppData")
}

pub(crate) fn open(vol: &Volume, disk: &mut dyn BlockDevice) -> Result<Tx, BtrfsError> {
    let mut superblock = alloc::vec![0u8; 4096];
    read_bytes(disk, SUPERBLOCK_OFFSET, &mut superblock)?;
    let root_addr = u64_at(&superblock, 80);
    let root = load_leaf(vol, disk, root_addr)?;
    let fs_addr = root_bytenr(&root, 5)?;
    let extent_addr = root_bytenr(&root, 2)?;
    let space_addr = root_bytenr(&root, 10)?;
    let csum_addr = root_bytenr(&root, 7)?;
    let fs = load_leaf(vol, disk, fs_addr)?;
    let extent = load_leaf(vol, disk, extent_addr)?;
    let space = load_leaf(vol, disk, space_addr)?;
    let csum = load_leaf(vol, disk, csum_addr)?;
    let mut groups = Vec::new();
    for (key, blob) in &extent.items {
        if key.typ == BLOCK_GROUP && blob.len() >= 24 {
            groups.push(Bg {
                start: key.obj,
                len: key.off,
                flags: u64_at(blob, 16),
            });
        }
    }
    let mut free = Vec::new();
    let mut info = Vec::new();
    for (key, blob) in &space.items {
        if key.typ == FREE_EXTENT {
            free.push((key.obj, key.off));
        } else if key.typ == FREE_INFO {
            info.push((key.obj, blob.clone()));
        }
    }
    free = clip_merge(&groups, free);
    Ok(Tx {
        nodesize: vol.nodesize,
        generation: u64_at(&superblock, 72).saturating_add(1),
        superblock,
        fs,
        extent,
        space,
        root,
        csum,
        fs_addr,
        extent_addr,
        space_addr,
        root_addr,
        csum_addr,
        groups,
        free,
        info,
        dirty: false,
        csum_dirty: false,
        data_delta: 0,
        payload: None,
        retired: Vec::new(),
    })
}

impl Tx {
    pub(crate) fn create_file(&mut self, path: &str) -> Result<(), BtrfsError> {
        self.ensure(path)?;
        if self.lookup(path).is_ok() {
            return Ok(());
        }
        let (parent, name) = split_path(path)?;
        let parent = self.lookup(parent)?;
        if !self.is_dir(parent)? {
            return Err(BtrfsError::NotDirectory);
        }
        let inode = self.alloc_inode();
        self.install_inode(inode, false, 0)?;
        self.link(parent, inode, name, FT_FILE)?;
        self.dirty = true;
        Ok(())
    }

    pub(crate) fn mkdir(&mut self, path: &str) -> Result<(), BtrfsError> {
        self.ensure(path)?;
        if self.lookup(path).is_ok() {
            return Err(BtrfsError::Exists);
        }
        let (parent, name) = split_path(path)?;
        let parent = self.lookup(parent)?;
        if !self.is_dir(parent)? {
            return Err(BtrfsError::NotDirectory);
        }
        let inode = self.alloc_inode();
        self.install_inode(inode, true, 0)?;
        self.link(parent, inode, name, FT_DIR)?;
        self.dirty = true;
        Ok(())
    }

    pub(crate) fn unlink(&mut self, path: &str) -> Result<(), BtrfsError> {
        self.ensure(path)?;
        let (parent_path, name) = split_path(path)?;
        let parent = self.lookup(parent_path)?;
        let inode = self.lookup(path)?;
        if self.is_dir(inode)? {
            if self.has_children(inode) {
                return Err(BtrfsError::NotEmpty);
            }
        }
        self.unlink_name(parent, name)?;
        self.drop_inode(inode)?;
        self.dirty = true;
        Ok(())
    }

    pub(crate) fn rename(&mut self, from: &str, to: &str) -> Result<(), BtrfsError> {
        self.ensure(from)?;
        self.ensure(to)?;
        if trim_slash(from) == trim_slash(to) {
            return Ok(());
        }
        let (src_parent_path, src_name) = split_path(from)?;
        let (dst_parent_path, dst_name) = split_path(to)?;
        let src_parent = self.lookup(src_parent_path)?;
        let dst_parent = self.lookup(dst_parent_path)?;
        if !self.is_dir(dst_parent)? {
            return Err(BtrfsError::NotDirectory);
        }
        let inode = self.lookup(from)?;
        if let Ok(dest) = self.lookup(to) {
            if self.is_dir(dest)? {
                return Err(BtrfsError::Exists);
            }
            self.unlink(to)?;
        }
        let kind = if self.is_dir(inode)? { FT_DIR } else { FT_FILE };
        self.unlink_name(src_parent, src_name)?;
        self.link(dst_parent, inode, dst_name, kind)?;
        self.dirty = true;
        Ok(())
    }

    pub(crate) fn write_path(
        &mut self,
        vol: &Volume,
        disk: &mut dyn BlockDevice,
        path: &str,
        data: &[u8],
        append: bool,
    ) -> Result<(), BtrfsError> {
        self.ensure(path)?;
        if self.lookup(path).is_err() {
            self.create_file(path)?;
        }
        let inode = self.lookup(path)?;
        self.write_at(vol, disk, inode, 0, data, append, !append)
    }

    pub(crate) fn write_at(
        &mut self,
        vol: &Volume,
        disk: &mut dyn BlockDevice,
        inode: u64,
        offset: u64,
        data: &[u8],
        append: bool,
        truncate: bool,
    ) -> Result<(), BtrfsError> {
        if self.is_dir(inode)? {
            return Err(BtrfsError::NotDirectory);
        }
        let mut body = if truncate {
            Vec::new()
        } else {
            self.read_inode(vol, disk, inode)?
        };
        let at = if append { body.len() as u64 } else { offset };
        let end = at as usize + data.len();
        if body.len() < end {
            body.resize(end, 0);
        }
        if !data.is_empty() {
            body[at as usize..end].copy_from_slice(data);
        }
        self.store_bytes(inode, body)?;
        self.dirty = true;
        Ok(())
    }

    pub(crate) fn commit(
        &mut self,
        vol: &Volume,
        disk: &mut dyn BlockDevice,
        write_super: bool,
    ) -> Result<u64, BtrfsError> {
        if !self.dirty {
            return Ok(self.fs_addr);
        }
        let mut meta = alloc::vec![
            (self.fs_addr, 5u64),
            (self.extent_addr, 2),
            (self.space_addr, 10),
            (self.root_addr, 1),
        ];
        if self.csum_dirty {
            meta.push((self.csum_addr, 7));
        }
        if self.payload.is_some() {
            let inode = self.payload.as_ref().unwrap().inode;
            let bytes = self.payload.as_ref().unwrap().bytes.clone();
            let disk_len = align_up(bytes.len() as u64, 4096);
            let addr = self.alloc(disk_len, 4096, BG_DATA)?;
            self.data_delta += disk_len as i64;
            if let Some(payload) = self.payload.as_mut() {
                payload.addr = addr;
                payload.disk_len = disk_len;
            }
            self.insert_file_extent(inode, &bytes, addr, disk_len)?;
        }
        let mut fresh = Vec::new();
        for (_, owner) in &meta {
            fresh.push((self.alloc(self.nodesize as u64, self.nodesize as u64, BG_META)?, *owner));
        }
        let old: Vec<u64> = meta.iter().map(|(addr, _)| *addr).collect();
        for addr in &old {
            self.free.push((*addr, self.nodesize as u64));
        }
        for (addr, len) in &self.retired {
            self.free.push((*addr, *len));
            self.data_delta -= *len as i64;
        }
        self.touch_data_group();
        self.free = clip_merge(&self.groups, core::mem::take(&mut self.free));
        let new_fs = fresh[0].0;
        let new_ext = fresh[1].0;
        let new_space = fresh[2].0;
        let new_root = fresh[3].0;
        let new_csum = fresh.get(4).map(|item| item.0);

        self.rebuild_extent(&fresh, &old)?;
        self.rebuild_space()?;
        self.patch_root(5, new_fs);
        self.patch_root(2, new_ext);
        self.patch_root(10, new_space);
        if let Some(addr) = new_csum {
            self.patch_root(7, addr);
        }
        let fs_node = self.fs.pack(new_fs, self.generation)?;
        let ext_node = self.extent.pack(new_ext, self.generation)?;
        let space_node = self.space.pack(new_space, self.generation)?;
        let root_node = self.root.pack(new_root, self.generation)?;
        write_phys(vol, disk, new_fs, &fs_node)?;
        write_phys(vol, disk, new_ext, &ext_node)?;
        write_phys(vol, disk, new_space, &space_node)?;
        write_phys(vol, disk, new_root, &root_node)?;
        if let Some(addr) = new_csum {
            let node = self.csum.pack(addr, self.generation)?;
            write_phys(vol, disk, addr, &node)?;
        }
        if let Some(payload) = &self.payload {
            let mut padded = payload.bytes.clone();
            padded.resize(payload.disk_len as usize, 0);
            write_phys(vol, disk, payload.addr, &padded)?;
        }
        disk.sync().map_err(|_| BtrfsError::Io)?;
        if write_super {
            put_u64(&mut self.superblock, 72, self.generation);
            put_u64(&mut self.superblock, 80, new_root);
            let used = u64_at(&self.superblock, 120) as i64 + self.data_delta;
            put_u64(&mut self.superblock, 120, used.max(0) as u64);
            checksum(&mut self.superblock);
            disk.write_blocks(SUPERBLOCK_OFFSET / 512, &self.superblock)
                .map_err(|_| BtrfsError::Io)?;
            disk.sync().map_err(|_| BtrfsError::Io)?;
        }
        Ok(new_fs)
    }

    fn ensure(&self, path: &str) -> Result<(), BtrfsError> {
        if may_write(path) {
            Ok(())
        } else {
            Err(BtrfsError::ReadOnly)
        }
    }

    fn lookup(&self, path: &str) -> Result<u64, BtrfsError> {
        let mut inode = 256u64;
        for part in trim_slash(path).split('/').filter(|p| !p.is_empty()) {
            inode = self.child(inode, part.as_bytes()).ok_or(BtrfsError::NotFound)?;
        }
        Ok(inode)
    }

    fn child(&self, parent: u64, name: &[u8]) -> Option<u64> {
        let hash = name_hash(name);
        let blob = self.fs.find(Key { obj: parent, typ: super::DIR_ITEM, off: hash })?;
        let mut p = 0usize;
        while p + 30 <= blob.len() {
            let data_len = u16_at(blob, p + 25) as usize;
            let name_len = u16_at(blob, p + 27) as usize;
            let start = p + 30 + data_len;
            let end = start + name_len;
            if end > blob.len() {
                break;
            }
            if &blob[start..end] == name {
                return Some(u64_at(blob, p));
            }
            p = end;
        }
        None
    }

    fn is_dir(&self, inode: u64) -> Result<bool, BtrfsError> {
        let blob = self.fs.find(Key { obj: inode, typ: super::INODE_ITEM, off: 0 })
            .ok_or(BtrfsError::NotFound)?;
        Ok(u32_at(blob, 52) & 0o170000 == 0o040000)
    }

    fn has_children(&self, inode: u64) -> bool {
        self.fs.items.iter().any(|(key, _)| key.obj == inode && key.typ == super::DIR_INDEX)
    }

    fn alloc_inode(&self) -> u64 {
        self.fs.items.iter().filter(|(key, _)| key.typ == super::INODE_ITEM).map(|(key, _)| key.obj).max().unwrap_or(256).saturating_add(1)
    }

    fn install_inode(&mut self, inode: u64, directory: bool, size: u64) -> Result<(), BtrfsError> {
        let mode_bits = if directory { 0o040000 } else { 0o100000 };
        let template = self.fs.items.iter().find_map(|(key, blob)| {
            if key.typ == super::INODE_ITEM && u32_at(blob, 52) & 0o170000 == mode_bits {
                Some(blob.clone())
            } else {
                None
            }
        }).ok_or(BtrfsError::Unsupported)?;
        let mut inode_item = template;
        put_u64(&mut inode_item, 0, self.generation);
        put_u64(&mut inode_item, 8, self.generation);
        put_u64(&mut inode_item, 16, size);
        put_u64(&mut inode_item, 24, size);
        put_u32(&mut inode_item, 40, 1);
        self.fs.insert(Key { obj: inode, typ: super::INODE_ITEM, off: 0 }, inode_item);
        Ok(())
    }

    fn link(&mut self, parent: u64, inode: u64, name: &str, kind: u8) -> Result<(), BtrfsError> {
        let index = self.fs.items.iter().filter(|(key, _)| key.obj == parent && key.typ == super::DIR_INDEX).map(|(key, _)| key.off).max().unwrap_or(0).saturating_add(1);
        let mut iref = Vec::new();
        iref.extend_from_slice(&index.to_le_bytes());
        iref.extend_from_slice(&(name.len() as u16).to_le_bytes());
        iref.extend_from_slice(name.as_bytes());
        self.fs.insert(Key { obj: inode, typ: INODE_REF, off: parent }, iref);
        let rec = dir_record(inode, name, kind, self.generation);
        let hash = name_hash(name.as_bytes());
        self.fs.append_item(Key { obj: parent, typ: super::DIR_ITEM, off: hash }, rec.clone());
        self.fs.insert(Key { obj: parent, typ: super::DIR_INDEX, off: index }, rec);
        self.bump_isize(parent, (name.len() as i64) * 2);
        Ok(())
    }

    fn unlink_name(&mut self, parent: u64, name: &str) -> Result<(), BtrfsError> {
        let hash = name_hash(name.as_bytes());
        let key = Key { obj: parent, typ: super::DIR_ITEM, off: hash };
        let Some(blob) = self.fs.find(key).map(|b| b.to_vec()) else {
            return Err(BtrfsError::NotFound);
        };
        let mut kept = Vec::new();
        let mut found = false;
        let mut p = 0usize;
        while p + 30 <= blob.len() {
            let data_len = u16_at(&blob, p + 25) as usize;
            let name_len = u16_at(&blob, p + 27) as usize;
            let start = p + 30 + data_len;
            let end = start + name_len;
            if end > blob.len() {
                break;
            }
            if &blob[start..end] == name.as_bytes() {
                found = true;
            } else {
                kept.extend_from_slice(&blob[p..end]);
            }
            p = end;
        }
        if !found {
            return Err(BtrfsError::NotFound);
        }
        if kept.is_empty() {
            self.fs.remove(key);
        } else {
            self.fs.insert(key, kept);
        }
        let index_key = self.fs.items.iter().find_map(|(key, blob)| {
            if key.obj == parent && key.typ == super::DIR_INDEX && record_name(blob) == Some(name) {
                Some(*key)
            } else {
                None
            }
        });
        if let Some(key) = index_key {
            let inode = u64_at(self.fs.find(key).unwrap_or(&[]), 0);
            self.fs.remove(key);
            self.fs.remove(Key { obj: inode, typ: INODE_REF, off: parent });
        }
        self.bump_isize(parent, -((name.len() as i64) * 2));
        Ok(())
    }

    fn drop_inode(&mut self, inode: u64) -> Result<(), BtrfsError> {
        let extents: Vec<(Key, Vec<u8>)> = self.fs.items.iter().filter(|(key, _)| key.obj == inode && key.typ == super::EXTENT_DATA).map(|(k, v)| (*k, v.clone())).collect();
        for (key, blob) in extents {
            if blob.len() > 45 && blob[20] == 1 {
                let addr = u64_at(&blob, 21);
                let len = u64_at(&blob, 29);
                self.retired.push((addr, len));
                self.extent.remove(Key { obj: addr, typ: DATA_ITEM, off: len });
                self.csum.remove(Key { obj: CSUM_OBJECT, typ: CSUM_ITEM, off: addr });
                self.csum_dirty = true;
            }
            self.fs.remove(key);
        }
        let doomed: Vec<Key> = self.fs.items.iter().filter(|(key, _)| key.obj == inode).map(|(key, _)| *key).collect();
        for key in doomed {
            self.fs.remove(key);
        }
        Ok(())
    }

    fn read_inode(&self, vol: &Volume, disk: &mut dyn BlockDevice, inode: u64) -> Result<Vec<u8>, BtrfsError> {
        let inode_item = self.fs.find(Key { obj: inode, typ: super::INODE_ITEM, off: 0 }).ok_or(BtrfsError::NotFound)?;
        let size = u64_at(inode_item, 16) as usize;
        if size == 0 {
            return Ok(Vec::new());
        }
        let mut out = alloc::vec![0u8; size];
        for (key, blob) in &self.fs.items {
            if key.obj != inode || key.typ != super::EXTENT_DATA || blob.len() < 21 {
                continue;
            }
            if blob[16] != 0 {
                return Err(BtrfsError::Compressed);
            }
            if blob[20] == 0 {
                let data = &blob[21..];
                let start = key.off as usize;
                let n = data.len().min(size.saturating_sub(start));
                if start < size {
                    out[start..start + n].copy_from_slice(&data[..n]);
                }
            } else if blob[20] == 1 {
                let disk_bytenr = u64_at(blob, 21);
                let extent_off = u64_at(blob, 37);
                let num_bytes = u64_at(blob, 45) as usize;
                let start = key.off as usize;
                let n = num_bytes.min(size.saturating_sub(start));
                if start < size && n > 0 {
                    read_bytes(disk, vol.map(disk_bytenr + extent_off)?, &mut out[start..start + n])?;
                }
            }
        }
        Ok(out)
    }

    fn store_bytes(&mut self, inode: u64, body: Vec<u8>) -> Result<(), BtrfsError> {
        self.drop_extents_keep_inode(inode)?;
        if let Some(item) = self.fs.find_mut(Key { obj: inode, typ: super::INODE_ITEM, off: 0 }) {
            put_u64(item, 0, self.generation);
            put_u64(item, 8, self.generation);
            put_u64(item, 16, body.len() as u64);
            put_u64(item, 24, body.len() as u64);
        }
        if body.is_empty() {
            return Ok(());
        }
        if body.len() <= INLINE_MAX {
            let mut ext = alloc::vec![0u8; 21 + body.len()];
            put_u64(&mut ext, 0, self.generation);
            put_u64(&mut ext, 8, body.len() as u64);
            ext[21..].copy_from_slice(&body);
            self.fs.insert(Key { obj: inode, typ: super::EXTENT_DATA, off: 0 }, ext);
        } else {
            self.payload = Some(Payload { inode, bytes: body, addr: 0, disk_len: 0 });
            self.csum_dirty = true;
        }
        Ok(())
    }

    fn drop_extents_keep_inode(&mut self, inode: u64) -> Result<(), BtrfsError> {
        let keys: Vec<(Key, Vec<u8>)> = self.fs.items.iter().filter(|(key, _)| key.obj == inode && key.typ == super::EXTENT_DATA).map(|(k, v)| (*k, v.clone())).collect();
        for (key, blob) in keys {
            if blob.len() > 20 && blob[16] != 0 {
                return Err(BtrfsError::Compressed);
            }
            if blob.len() > 45 && blob[20] == 1 {
                let addr = u64_at(&blob, 21);
                let len = u64_at(&blob, 29);
                self.retired.push((addr, len));
                self.extent.remove(Key { obj: addr, typ: DATA_ITEM, off: len });
                self.csum.remove(Key { obj: CSUM_OBJECT, typ: CSUM_ITEM, off: addr });
                self.csum_dirty = true;
            }
            self.fs.remove(key);
        }
        Ok(())
    }

    fn insert_file_extent(&mut self, inode: u64, bytes: &[u8], addr: u64, disk_len: u64) -> Result<(), BtrfsError> {
        let mut ext = alloc::vec![0u8; 53];
        put_u64(&mut ext, 0, self.generation);
        put_u64(&mut ext, 8, disk_len);
        ext[20] = 1;
        put_u64(&mut ext, 21, addr);
        put_u64(&mut ext, 29, disk_len);
        put_u64(&mut ext, 37, 0);
        put_u64(&mut ext, 45, disk_len);
        let _ = bytes;
        if let Some(item) = self.fs.find_mut(Key { obj: inode, typ: super::INODE_ITEM, off: 0 }) {
            put_u64(item, 24, disk_len);
        }
        self.fs.insert(Key { obj: inode, typ: super::EXTENT_DATA, off: 0 }, ext);
        let mut item = alloc::vec![0u8; 53];
        put_u64(&mut item, 0, 1);
        put_u64(&mut item, 8, self.generation);
        put_u64(&mut item, 16, FLAG_DATA);
        item[24] = 178;
        put_u64(&mut item, 25, 5);
        put_u64(&mut item, 33, inode);
        put_u64(&mut item, 41, 0);
        put_u32(&mut item, 49, 1);
        self.extent.insert(Key { obj: addr, typ: DATA_ITEM, off: disk_len }, item);
        let mut sum = Vec::new();
        let mut padded = bytes.to_vec();
        padded.resize(disk_len as usize, 0);
        for chunk in padded.chunks(4096) {
            sum.extend_from_slice(&checksum_value(chunk).to_le_bytes());
        }
        self.csum.insert(Key { obj: CSUM_OBJECT, typ: CSUM_ITEM, off: addr }, sum);
        Ok(())
    }

    fn touch_data_group(&mut self) {
        if self.data_delta == 0 {
            return;
        }
        let Some(bg) = self.groups.iter().find(|bg| bg.flags & BG_DATA != 0).copied() else {
            return;
        };
        if let Some(group) = self.extent.find_mut(Key { obj: bg.start, typ: BLOCK_GROUP, off: bg.len }) {
            let used = (u64_at(group, 0) as i64 + self.data_delta).max(0) as u64;
            put_u64(group, 0, used);
        }
    }

    fn bump_isize(&mut self, inode: u64, delta: i64) {
        if let Some(blob) = self.fs.find_mut(Key { obj: inode, typ: super::INODE_ITEM, off: 0 }) {
            let size = u64_at(blob, 16) as i64 + delta;
            put_u64(blob, 16, size.max(0) as u64);
        }
    }

    fn alloc(&mut self, len: u64, align: u64, flags: u64) -> Result<u64, BtrfsError> {
        self.free = clip_merge(&self.groups, core::mem::take(&mut self.free));
        let mut chosen = None;
        for (index, (start, span)) in self.free.iter().copied().enumerate() {
            let aligned = align_up(start, align);
            if aligned < start || aligned + len > start + span {
                continue;
            }
            let inside = self.groups.iter().any(|bg| bg.flags & flags != 0 && bg.start <= aligned && aligned < bg.start + bg.len);
            if !inside {
                continue;
            }
            chosen = Some((index, start, span, aligned));
            break;
        }
        let Some((index, start, span, aligned)) = chosen else {
            return Err(BtrfsError::Io);
        };
        self.free.remove(index);
        if aligned > start {
            self.free.push((start, aligned - start));
        }
        if aligned + len < start + span {
            self.free.push((aligned + len, start + span - (aligned + len)));
        }
        self.free = clip_merge(&self.groups, core::mem::take(&mut self.free));
        Ok(aligned)
    }

    fn rebuild_extent(&mut self, fresh: &[(u64, u64)], old: &[u64]) -> Result<(), BtrfsError> {
        self.extent.items.retain(|(key, _)| !(key.typ == META_ITEM && old.contains(&key.obj)));
        self.extent.items.retain(|(key, _)| !(key.typ == DATA_ITEM && self.retired.iter().any(|(addr, _)| *addr == key.obj)));
        for (addr, owner) in fresh {
            let mut blob = alloc::vec![0u8; 33];
            put_u64(&mut blob, 0, 1);
            put_u64(&mut blob, 8, self.generation);
            put_u64(&mut blob, 16, FLAG_TREE);
            blob[24] = 0xB0;
            put_u64(&mut blob, 25, *owner);
            self.extent.insert(Key { obj: *addr, typ: META_ITEM, off: 0 }, blob);
        }
        self.extent.items.sort_by_key(|(key, _)| *key);
        Ok(())
    }

    fn rebuild_space(&mut self) -> Result<(), BtrfsError> {
        let mut items = Vec::new();
        for group in &self.groups {
            let mut extents: Vec<(u64, u64)> = self.free.iter().copied().filter(|(start, len)| group.start <= *start && start + len <= group.start + group.len).collect();
            extents.sort();
            let mut info = self.info.iter().find(|(start, _)| *start == group.start).map(|(_, blob)| blob.clone()).unwrap_or_else(|| alloc::vec![0u8; 8]);
            put_u32(&mut info, 0, extents.len() as u32);
            items.push((Key { obj: group.start, typ: FREE_INFO, off: group.len }, info));
            for (start, len) in extents {
                items.push((Key { obj: start, typ: FREE_EXTENT, off: len }, Vec::new()));
            }
        }
        items.sort_by_key(|(key, _)| *key);
        self.space.items = items;
        Ok(())
    }

    fn patch_root(&mut self, obj: u64, bytenr: u64) {
        for (key, blob) in &mut self.root.items {
            if key.obj == obj && key.typ == super::ROOT_ITEM && blob.len() >= 200 {
                put_u64(blob, 0, self.generation);
                put_u64(blob, 160, self.generation);
                put_u64(blob, 176, bytenr);
                put_u64(blob, 192, self.nodesize as u64);
            }
        }
    }
}

fn load_leaf(vol: &Volume, disk: &mut dyn BlockDevice, logical: u64) -> Result<Leaf, BtrfsError> {
    let mut node = alloc::vec![0u8; vol.nodesize];
    read_bytes(disk, vol.map(logical)?, &mut node)?;
    if node[100] != 0 {
        return Err(BtrfsError::Unsupported);
    }
    Leaf::parse(&node)
}

fn root_bytenr(root: &Leaf, obj: u64) -> Result<u64, BtrfsError> {
    let blob = root.find(Key { obj, typ: super::ROOT_ITEM, off: 0 }).or_else(|| {
        root.items.iter().find(|(key, _)| key.obj == obj && key.typ == super::ROOT_ITEM).map(|(_, blob)| blob.as_slice())
    }).ok_or(BtrfsError::NotFound)?;
    if blob.len() < 184 {
        return Err(BtrfsError::Io);
    }
    Ok(u64_at(blob, 176))
}

fn write_phys(vol: &Volume, disk: &mut dyn BlockDevice, logical: u64, buf: &[u8]) -> Result<(), BtrfsError> {
    let phys = vol.map(logical)?;
    if phys % 512 != 0 || buf.len() % 512 != 0 {
        return Err(BtrfsError::Io);
    }
    disk.write_blocks(phys / 512, buf).map_err(|_| BtrfsError::Io)
}

impl Leaf {
    fn parse(node: &[u8]) -> Result<Self, BtrfsError> {
        let nodesize = node.len();
        let n = u32_at(node, 96) as usize;
        let mut items = Vec::new();
        for i in 0..n {
            let at = 101 + i * 25;
            if at + 25 > node.len() {
                return Err(BtrfsError::Io);
            }
            let key = Key { obj: u64_at(node, at), typ: node[at + 8], off: u64_at(node, at + 9) };
            let data_off = u32_at(node, at + 17) as usize + 101;
            let data_len = u32_at(node, at + 21) as usize;
            if data_off + data_len > node.len() {
                return Err(BtrfsError::Io);
            }
            items.push((key, node[data_off..data_off + data_len].to_vec()));
        }
        Ok(Self { header: node[..101].to_vec(), items, nodesize })
    }

    fn pack(&self, bytenr: u64, generation: u64) -> Result<Vec<u8>, BtrfsError> {
        let mut node = alloc::vec![0u8; self.nodesize];
        let n = self.header.len().min(101);
        node[..n].copy_from_slice(&self.header[..n]);
        put_u64(&mut node, 48, bytenr);
        put_u64(&mut node, 80, generation);
        put_u32(&mut node, 96, self.items.len() as u32);
        node[100] = 0;
        let headers_end = 101 + self.items.len() * 25;
        let mut cursor = self.nodesize;
        for (index, (key, blob)) in self.items.iter().enumerate() {
            if cursor < blob.len() || cursor - blob.len() < headers_end {
                return Err(BtrfsError::Unsupported);
            }
            cursor -= blob.len();
            node[cursor..cursor + blob.len()].copy_from_slice(blob);
            let at = 101 + index * 25;
            put_u64(&mut node, at, key.obj);
            node[at + 8] = key.typ;
            put_u64(&mut node, at + 9, key.off);
            put_u32(&mut node, at + 17, (cursor - 101) as u32);
            put_u32(&mut node, at + 21, blob.len() as u32);
        }
        checksum(&mut node);
        Ok(node)
    }

    fn find(&self, key: Key) -> Option<&[u8]> {
        self.items.iter().find(|(item, _)| *item == key).map(|(_, blob)| blob.as_slice())
    }

    fn find_mut(&mut self, key: Key) -> Option<&mut Vec<u8>> {
        self.items.iter_mut().find(|(item, _)| *item == key).map(|(_, blob)| blob)
    }

    fn insert(&mut self, key: Key, data: Vec<u8>) {
        if let Some((_, blob)) = self.items.iter_mut().find(|(item, _)| *item == key) {
            *blob = data;
        } else {
            self.items.push((key, data));
        }
        self.items.sort_by_key(|(item, _)| *item);
    }

    fn append_item(&mut self, key: Key, extra: Vec<u8>) {
        if let Some((_, blob)) = self.items.iter_mut().find(|(item, _)| *item == key) {
            blob.extend_from_slice(&extra);
        } else {
            self.items.push((key, extra));
            self.items.sort_by_key(|(item, _)| *item);
        }
    }

    fn remove(&mut self, key: Key) {
        self.items.retain(|(item, _)| *item != key);
    }
}

fn dir_record(inode: u64, name: &str, kind: u8, generation: u64) -> Vec<u8> {
    let mut rec = Vec::new();
    rec.extend_from_slice(&inode.to_le_bytes());
    rec.push(1);
    rec.extend_from_slice(&0u64.to_le_bytes());
    rec.extend_from_slice(&generation.to_le_bytes());
    rec.extend_from_slice(&0u16.to_le_bytes());
    rec.extend_from_slice(&(name.len() as u16).to_le_bytes());
    rec.push(kind);
    rec.extend_from_slice(name.as_bytes());
    rec
}

fn record_name(blob: &[u8]) -> Option<&str> {
    if blob.len() < 30 {
        return None;
    }
    let data_len = u16_at(blob, 25) as usize;
    let name_len = u16_at(blob, 27) as usize;
    let start = 30 + data_len;
    let end = start + name_len;
    if end > blob.len() {
        return None;
    }
    core::str::from_utf8(&blob[start..end]).ok()
}

fn clip_merge(groups: &[Bg], free: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    let mut parts = Vec::new();
    for (start, len) in free {
        let end = start + len;
        for group in groups {
            let a = start.max(group.start);
            let b = end.min(group.start + group.len);
            if a < b {
                parts.push((a, b - a));
            }
        }
    }
    parts.sort();
    let mut merged = Vec::new();
    for (start, len) in parts {
        if let Some((prev, plen)) = merged.last_mut() {
            if *prev + *plen == start && bg_of(groups, *prev) == bg_of(groups, start) {
                *plen += len;
                continue;
            }
        }
        merged.push((start, len));
    }
    merged
}

fn bg_of(groups: &[Bg], addr: u64) -> Option<u64> {
    groups.iter().find(|bg| bg.start <= addr && addr < bg.start + bg.len).map(|bg| bg.start)
}

fn align_up(value: u64, align: u64) -> u64 {
    if align == 0 { return value; }
    (value + align - 1) & !(align - 1)
}

fn trim_slash(path: &str) -> &str {
    path.trim_matches('/')
}

fn split_path(path: &str) -> Result<(&str, &str), BtrfsError> {
    let path = path.trim_end_matches('/');
    if path.is_empty() || path == "/" {
        return Err(BtrfsError::InvalidPath);
    }
    match path.rfind('/') {
        Some(0) => Ok(("/", &path[1..])),
        Some(index) => Ok((&path[..index], &path[index + 1..])),
        None => Err(BtrfsError::InvalidPath),
    }.and_then(|(parent, name)| {
        if name.is_empty() || name == "." || name == ".." {
            Err(BtrfsError::InvalidPath)
        } else {
            Ok((parent, name))
        }
    })
}

fn checksum(buf: &mut [u8]) {
    if buf.len() < 32 {
        return;
    }
    buf[..32].fill(0);
    let crc = checksum_value(&buf[32..]);
    buf[..4].copy_from_slice(&crc.to_le_bytes());
}

fn checksum_value(buf: &[u8]) -> u32 {
    let mut crc = 0xFFFFFFFFu32;
    for &byte in buf {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0x82F6_3B78 } else { crc >> 1 };
        }
    }
    crc ^ 0xFFFF_FFFF
}

fn put_u64(buf: &mut [u8], offset: usize, value: u64) {
    if offset + 8 <= buf.len() {
        buf[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
}
fn put_u32(buf: &mut [u8], offset: usize, value: u32) {
    if offset + 4 <= buf.len() {
        buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
}
