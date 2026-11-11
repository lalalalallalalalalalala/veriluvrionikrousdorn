//! Btrfs: чтение и копирующая запись.
//!
//! Запись не затирает живые блоки. Новые узлы деревьев пишутся первыми,
//! суперблок — последним, поэтому обрыв посередине оставляет прежнее
//! согласованное состояние. Сжатый экстент по-прежнему ошибка.
//!
//! Разбираются суперблок, массив системных чанков и дерево чанков (логический
//! адрес в физический), корневое дерево, дерево файлов, каталоги, иноды и
//! экстенты файлов, включая inline. Сжатый экстент возвращает
//! [`BtrfsError::Compressed`]. Запись не реализована.

#![no_std]

extern crate alloc;

mod commit;

use alloc::vec::Vec;
use vlr_drivers::block::{BlockDevice, BlockError};

pub const SUPERBLOCK_OFFSET: u64 = 0x10000;
pub const MAGIC: &[u8; 8] = b"_BHRfS_M";

const HEADER: usize = 101;
const INODE_ITEM: u8 = 1;
const DIR_ITEM: u8 = 84;
const DIR_INDEX: u8 = 96;
const EXTENT_DATA: u8 = 108;
const ROOT_ITEM: u8 = 132;
const CHUNK_ITEM: u8 = 228;
const FS_TREE: u64 = 5;
const ROOT_DIR: u64 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtrfsError {
    Io,
    BadMagic,
    NotFound,
    NotDirectory,
    NotFile,
    NotEmpty,
    Exists,
    ReadOnly,
    InvalidPath,
    Compressed,
    Unsupported,
}

#[derive(Clone, Copy)]
struct Chunk {
    logical: u64,
    length: u64,
    physical: u64,
}

pub struct Volume {
    nodesize: usize,
    fs_root: u64,
    chunks: Vec<Chunk>,
}

pub fn mount(disk: &mut dyn BlockDevice) -> Result<Volume, BtrfsError> {
    let mut sb = [0u8; 4096];
    read_bytes(disk, SUPERBLOCK_OFFSET, &mut sb)?;
    if &sb[64..72] != MAGIC {
        return Err(BtrfsError::BadMagic);
    }
    let nodesize = u32_at(&sb, 148) as usize;
    if !(4096..=65536).contains(&nodesize) {
        return Err(BtrfsError::Unsupported);
    }
    let mut chunks = parse_sys_chunks(&sb)?;
    let chunk_root = u64_at(&sb, 88);
    absorb_chunk_tree(disk, &mut chunks, chunk_root, nodesize)?;
    let root = u64_at(&sb, 80);
    let fs_root = find_fs_root(disk, &chunks, root, nodesize)?;
    Ok(Volume {
        nodesize,
        fs_root,
        chunks,
    })
}

impl Volume {
    pub fn lookup(&self, disk: &mut dyn BlockDevice, path: &str) -> Result<u64, BtrfsError> {
        let mut inode = ROOT_DIR;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            inode = self.dir_lookup(disk, inode, part)?;
        }
        Ok(inode)
    }

    pub fn is_dir(&self, disk: &mut dyn BlockDevice, inode: u64) -> Result<bool, BtrfsError> {
        let item = self.find(disk, self.fs_root, (inode, INODE_ITEM, 0))?;
        let mode = u32_at(&item, 52);
        Ok(mode & 0o170000 == 0o040000)
    }

    pub fn file_size(&self, disk: &mut dyn BlockDevice, inode: u64) -> Result<u64, BtrfsError> {
        let item = self.find(disk, self.fs_root, (inode, INODE_ITEM, 0))?;
        Ok(u64_at(&item, 16))
    }

    pub fn read_file(
        &self,
        disk: &mut dyn BlockDevice,
        inode: u64,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, BtrfsError> {
        let size = self.file_size(disk, inode)?;
        if offset >= size || buf.is_empty() {
            return Ok(0);
        }
        let want = buf.len().min((size - offset) as usize);
        let mut done = 0usize;
        while done < want {
            let pos = offset + done as u64;
            let (key_off, item) = self.find_extent(disk, inode, pos)?;
            let comp = item[16];
            let kind = item[20];
            if comp != 0 {
                return Err(BtrfsError::Compressed);
            }
            if kind == 0 {
                let data = &item[21..];
                let start = (pos - key_off) as usize;
                if start >= data.len() {
                    return Err(BtrfsError::Io);
                }
                let n = (want - done).min(data.len() - start);
                buf[done..done + n].copy_from_slice(&data[start..start + n]);
                done += n;
            } else if kind == 1 {
                let disk_bytenr = u64_at(&item, 21);
                let extent_off = u64_at(&item, 37);
                let num_bytes = u64_at(&item, 45);
                let within = pos - key_off;
                if within >= num_bytes {
                    return Err(BtrfsError::Io);
                }
                let n = (want - done).min((num_bytes - within) as usize);
                let logical = disk_bytenr + extent_off + within;
                read_bytes(disk, self.map(logical)?, &mut buf[done..done + n])?;
                done += n;
            } else {
                return Err(BtrfsError::Unsupported);
            }
        }
        Ok(done)
    }

    pub fn list_dir(
        &self,
        disk: &mut dyn BlockDevice,
        inode: u64,
    ) -> Result<Vec<(alloc::string::String, u64)>, BtrfsError> {
        if !self.is_dir(disk, inode)? {
            return Err(BtrfsError::NotDirectory);
        }
        let mut names = Vec::new();
        self.walk(disk, self.fs_root, &mut |obj, typ, _off, data| {
            if obj == inode && typ == DIR_INDEX {
                if let Some((name, child)) = dir_name(data) {
                    if name != "." && name != ".." {
                        names.push((alloc::string::String::from(name), child));
                    }
                }
            }
        })?;
        Ok(names)
    }

    fn dir_lookup(
        &self,
        disk: &mut dyn BlockDevice,
        dir: u64,
        name: &str,
    ) -> Result<u64, BtrfsError> {
        let hash = name_hash(name.as_bytes());
        let item = self.find(disk, self.fs_root, (dir, DIR_ITEM, hash))?;
        let mut p = 0usize;
        while p + 30 <= item.len() {
            let data_len = u16_at(&item, p + 25) as usize;
            let name_len = u16_at(&item, p + 27) as usize;
            let start = p + 30 + data_len;
            let end = start + name_len;
            if end > item.len() {
                break;
            }
            if &item[start..end] == name.as_bytes() {
                return Ok(u64_at(&item, p));
            }
            p = end;
        }
        Err(BtrfsError::NotFound)
    }

    fn find_extent(
        &self,
        disk: &mut dyn BlockDevice,
        inode: u64,
        file_off: u64,
    ) -> Result<(u64, Vec<u8>), BtrfsError> {
        let mut best: Option<(u64, Vec<u8>)> = None;
        self.walk(disk, self.fs_root, &mut |obj, typ, off, data| {
            if obj == inode && typ == EXTENT_DATA && off <= file_off {
                best = Some((off, data.to_vec()));
            }
        })?;
        best.ok_or(BtrfsError::NotFound)
    }

    fn find(
        &self,
        disk: &mut dyn BlockDevice,
        root: u64,
        key: (u64, u8, u64),
    ) -> Result<Vec<u8>, BtrfsError> {
        let mut found = None;
        self.walk(disk, root, &mut |obj, typ, off, data| {
            if found.is_none() && (obj, typ, off) == key {
                found = Some(data.to_vec());
            }
        })?;
        found.ok_or(BtrfsError::NotFound)
    }

    fn walk(
        &self,
        disk: &mut dyn BlockDevice,
        bytenr: u64,
        visit: &mut dyn FnMut(u64, u8, u64, &[u8]),
    ) -> Result<(), BtrfsError> {
        let mut node = alloc::vec![0u8; self.nodesize];
        read_bytes(disk, self.map(bytenr)?, &mut node)?;
        let level = node[100];
        let n = u32_at(&node, 96) as usize;
        if level == 0 {
            for i in 0..n {
                let at = HEADER + i * 25;
                let obj = u64_at(&node, at);
                let typ = node[at + 8];
                let off = u64_at(&node, at + 9);
                let data_off = u32_at(&node, at + 17) as usize + HEADER;
                let data_len = u32_at(&node, at + 21) as usize;
                if data_off + data_len > node.len() {
                    return Err(BtrfsError::Io);
                }
                visit(obj, typ, off, &node[data_off..data_off + data_len]);
            }
            return Ok(());
        }
        let mut children = Vec::new();
        for i in 0..n {
            let at = HEADER + i * 33;
            children.push(u64_at(&node, at + 17));
        }
        for child in children {
            self.walk(disk, child, visit)?;
        }
        Ok(())
    }

    fn map(&self, logical: u64) -> Result<u64, BtrfsError> {
        for chunk in &self.chunks {
            if logical >= chunk.logical && logical < chunk.logical + chunk.length {
                return Ok(chunk.physical + (logical - chunk.logical));
            }
        }
        Err(BtrfsError::Io)
    }
}

fn find_fs_root(
    disk: &mut dyn BlockDevice,
    chunks: &[Chunk],
    root: u64,
    nodesize: usize,
) -> Result<u64, BtrfsError> {
    let vol = Volume {
        nodesize,
        fs_root: 0,
        chunks: chunks.to_vec(),
    };
    let mut found = None;
    vol.walk(disk, root, &mut |obj, typ, _off, data| {
        if obj == FS_TREE && typ == ROOT_ITEM && data.len() >= 184 {
            found = Some(u64_at(data, 176));
        }
    })?;
    found.ok_or(BtrfsError::NotFound)
}

fn absorb_chunk_tree(
    disk: &mut dyn BlockDevice,
    chunks: &mut Vec<Chunk>,
    root: u64,
    nodesize: usize,
) -> Result<(), BtrfsError> {
    let vol = Volume {
        nodesize,
        fs_root: 0,
        chunks: chunks.clone(),
    };
    let mut extra = Vec::new();
    vol.walk(disk, root, &mut |_obj, typ, logical, data| {
        if typ == CHUNK_ITEM && data.len() >= 80 {
            let length = u64_at(data, 0);
            let physical = u64_at(data, 56);
            extra.push(Chunk {
                logical,
                length,
                physical,
            });
        }
    })?;
    for chunk in extra {
        if !chunks.iter().any(|c| c.logical == chunk.logical) {
            chunks.push(chunk);
        }
    }
    Ok(())
}

fn parse_sys_chunks(sb: &[u8]) -> Result<Vec<Chunk>, BtrfsError> {
    let size = u32_at(sb, 160) as usize;
    let mut off = 811usize;
    let end = off + size;
    let mut chunks = Vec::new();
    while off + 17 <= end && off + 17 <= sb.len() {
        let logical = u64_at(sb, off + 9);
        let c = off + 17;
        if c + 64 > sb.len() || c + 64 > end {
            break;
        }
        let length = u64_at(sb, c);
        let stripes = u16_at(sb, c + 44) as usize;
        let physical = u64_at(sb, c + 56);
        chunks.push(Chunk {
            logical,
            length,
            physical,
        });
        let step = 17 + 48 + stripes * 32;
        if step == 0 {
            break;
        }
        off += step;
    }
    if chunks.is_empty() {
        return Err(BtrfsError::Unsupported);
    }
    Ok(chunks)
}

fn dir_name(data: &[u8]) -> Option<(&str, u64)> {
    if data.len() < 30 {
        return None;
    }
    let child = u64_at(data, 0);
    let data_len = u16_at(data, 25) as usize;
    let name_len = u16_at(data, 27) as usize;
    let start = 30 + data_len;
    let end = start + name_len;
    if end > data.len() {
        return None;
    }
    let name = core::str::from_utf8(&data[start..end]).ok()?;
    Some((name, child))
}

fn name_hash(name: &[u8]) -> u64 {
    let mut crc = 0xFFFFFFFEu32;
    for &byte in name {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x82F6_3B78
            } else {
                crc >> 1
            };
        }
    }
    crc as u64
}

fn read_bytes(disk: &mut dyn BlockDevice, offset: u64, buf: &mut [u8]) -> Result<(), BtrfsError> {
    let mut done = 0usize;
    while done < buf.len() {
        let pos = offset + done as u64;
        let lba = pos / 512;
        let skip = (pos % 512) as usize;
        let mut sector = [0u8; 512];
        let n = (buf.len() - done).min(512 - skip);
        disk.read_blocks(lba, &mut sector).map_err(map_io)?;
        buf[done..done + n].copy_from_slice(&sector[skip..skip + n]);
        done += n;
    }
    Ok(())
}

fn map_io(err: BlockError) -> BtrfsError {
    match err {
        BlockError::Unsupported => BtrfsError::Unsupported,
        _ => BtrfsError::Io,
    }
}

use spin::Mutex;

static VOLUME: Mutex<Option<Volume>> = Mutex::new(None);

fn disk<R>(f: impl FnOnce(&mut dyn BlockDevice) -> Result<R, BtrfsError>) -> Result<R, BtrfsError> {
    match vlr_drivers::with_root_disk(|disk| f(disk)) {
        Ok(inner) => inner,
        Err(_) => Err(BtrfsError::Io),
    }
}

pub fn mount_global() -> Result<(), BtrfsError> {
    let volume = disk(mount)?;
    *VOLUME.lock() = Some(volume);
    Ok(())
}

pub fn read_path(path: &str) -> Result<Vec<u8>, BtrfsError> {
    let guard = VOLUME.lock();
    let vol = guard.as_ref().ok_or(BtrfsError::NotFound)?;
    disk(|disk| {
        let inode = vol.lookup(disk, path)?;
        let size = vol.file_size(disk, inode)?;
        if size > 2 * 1024 * 1024 {
            return Err(BtrfsError::Unsupported);
        }
        let mut buf = alloc::vec![0u8; size as usize];
        let n = vol.read_file(disk, inode, 0, &mut buf)?;
        buf.truncate(n);
        Ok(buf)
    })
}

pub fn lookup_inode(path: &str) -> Result<u64, BtrfsError> {
    let guard = VOLUME.lock();
    let vol = guard.as_ref().ok_or(BtrfsError::NotFound)?;
    disk(|disk| vol.lookup(disk, path))
}

pub fn read_at(inode: u64, offset: u64, buf: &mut [u8]) -> Result<usize, BtrfsError> {
    let guard = VOLUME.lock();
    let vol = guard.as_ref().ok_or(BtrfsError::NotFound)?;
    disk(|disk| vol.read_file(disk, inode, offset, buf))
}

pub fn list_inode(inode: u64) -> Result<Vec<alloc::string::String>, BtrfsError> {
    let guard = VOLUME.lock();
    let vol = guard.as_ref().ok_or(BtrfsError::NotFound)?;
    disk(|disk| {
        let mut names = vol.list_dir(disk, inode)?;
        names.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(names.into_iter().map(|(name, _)| name).collect())
    })
}

#[allow(dead_code)]
pub(crate) fn apply<R>(
    vol: &mut Volume,
    disk: &mut dyn BlockDevice,
    superblock: bool,
    f: impl FnOnce(&mut commit::Tx, &Volume, &mut dyn BlockDevice) -> Result<R, BtrfsError>,
) -> Result<R, BtrfsError> {
    let mut tx = commit::open(vol, disk)?;
    let value = f(&mut tx, vol, disk)?;
    if tx.dirty {
        let root = tx.commit(vol, disk, superblock)?;
        if superblock {
            vol.fs_root = root;
        }
    }
    Ok(value)
}

fn mutate<R>(
    f: impl FnOnce(&mut commit::Tx, &Volume, &mut dyn BlockDevice) -> Result<R, BtrfsError>,
) -> Result<R, BtrfsError> {
    let mut guard = VOLUME.lock();
    let vol = guard.as_mut().ok_or(BtrfsError::NotFound)?;
    disk(|disk| {
        let mut tx = commit::open(vol, disk)?;
        let value = f(&mut tx, vol, disk)?;
        if tx.dirty {
            vol.fs_root = tx.commit(vol, disk, true)?;
        }
        Ok(value)
    })
}

pub fn may_write(path: &str) -> bool {
    commit::may_write(path)
}

pub fn set_session(name: &str) {
    commit::set_session(name);
}

pub fn session_name() -> alloc::string::String {
    commit::session_name()
}

pub fn system_mkdir(path: &str) -> Result<(), BtrfsError> {
    if !privileged_path(path) {
        return Err(BtrfsError::ReadOnly);
    }
    mutate(|tx, _, _| {
        tx.elevate();
        match tx.mkdir(path) {
            Err(BtrfsError::Exists) => Ok(()),
            other => other,
        }
    })
}

fn privileged_path(path: &str) -> bool {
    path.starts_with("/System")
        || path.starts_with("/Users")
        || path.starts_with("/Apps")
        || path.starts_with("/Libs")
        || path.starts_with("/AppData")
}

pub fn system_remove(path: &str) -> Result<(), BtrfsError> {
    if !privileged_path(path) {
        return Err(BtrfsError::ReadOnly);
    }
    mutate(|tx, _, _| {
        tx.elevate();
        match tx.unlink(path) {
            Err(BtrfsError::NotFound) => Ok(()),
            other => other,
        }
    })
}

pub fn system_write(path: &str, data: &[u8]) -> Result<(), BtrfsError> {
    if !privileged_path(path) {
        return Err(BtrfsError::ReadOnly);
    }
    mutate(|tx, vol, disk| {
        tx.elevate();
        tx.write_path(vol, disk, path, data, false)
    })
}

pub fn create_file(path: &str) -> Result<(), BtrfsError> {
    mutate(|tx, _, _| tx.create_file(path))
}

pub fn make_dir(path: &str) -> Result<(), BtrfsError> {
    mutate(|tx, _, _| tx.mkdir(path))
}

pub fn remove_path(path: &str) -> Result<(), BtrfsError> {
    mutate(|tx, _, _| tx.unlink(path))
}

pub fn rename_path(from: &str, to: &str) -> Result<(), BtrfsError> {
    mutate(|tx, _, _| tx.rename(from, to))
}

pub fn write_inode(inode: u64, offset: u64, data: &[u8], append: bool, truncate: bool) -> Result<(), BtrfsError> {
    mutate(|tx, vol, disk| tx.write_at(vol, disk, inode, offset, data, append, truncate))
}

pub fn list_path(path: &str) -> Result<Vec<alloc::string::String>, BtrfsError> {
    let guard = VOLUME.lock();
    let vol = guard.as_ref().ok_or(BtrfsError::NotFound)?;
    disk(|disk| {
        let inode = vol.lookup(disk, path)?;
        let mut names = vol.list_dir(disk, inode)?;
        names.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(names.into_iter().map(|(name, _)| name).collect())
    })
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap_or([0; 8]))
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap_or([0; 4]))
}
fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap_or([0; 2]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vlr_drivers::block::BlockError;

    extern crate std;

    use std::process::Command;

    struct MemDisk(Vec<u8>);
    impl BlockDevice for MemDisk {
        fn block_size(&self) -> u64 {
            512
        }
        fn block_count(&self) -> u64 {
            self.0.len() as u64 / 512
        }
        fn read_blocks(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
            let off = (lba as usize) * 512;
            if off + buf.len() > self.0.len() {
                return Err(BlockError::OutOfRange);
            }
            buf.copy_from_slice(&self.0[off..off + buf.len()]);
            Ok(())
        }
        fn write_blocks(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
            let off = (lba as usize) * 512;
            if off + buf.len() > self.0.len() {
                return Err(BlockError::OutOfRange);
            }
            self.0[off..off + buf.len()].copy_from_slice(buf);
            Ok(())
        }
    }

    #[test]
    fn reads_files_from_mkfs_image() {
        let root = std::env::temp_dir().join("vlr-btrfs-test-root");
        let image = std::env::temp_dir().join("vlr-btrfs-test.img");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Apps/hello")).unwrap();
        std::fs::create_dir_all(root.join("Users/user")).unwrap();
        std::fs::write(root.join("Users/user/motd"), b"vlr-motd\n").unwrap();
        std::fs::write(root.join("Apps/hello/note.txt"), b"note-ok").unwrap();
        let bulky = std::vec![0x5Au8; 8192];
        std::fs::write(root.join("Apps/hello/bulky.bin"), &bulky).unwrap();
        std::fs::write(&image, std::vec![0u8; 32 * 1024 * 1024]).unwrap();
        let status = Command::new("mkfs.btrfs")
            .args([
                "-q",
                "-f",
                "-m",
                "single",
                "-d",
                "single",
                "-r",
                root.to_str().unwrap(),
                "--shrink",
                image.to_str().unwrap(),
            ])
            .status()
            .expect("mkfs.btrfs");
        assert!(status.success());
        let mut disk = MemDisk(std::fs::read(&image).unwrap());
        let vol = mount(&mut disk).unwrap();
        let motd = vol.lookup(&mut disk, "/Users/user/motd").unwrap();
        let mut buf = [0u8; 32];
        let n = vol.read_file(&mut disk, motd, 0, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"vlr-motd\n");
        let note = vol.lookup(&mut disk, "/Apps/hello/note.txt").unwrap();
        let n = vol.read_file(&mut disk, note, 0, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"note-ok");
        let apps = vol.lookup(&mut disk, "/Apps").unwrap();
        let names = vol.list_dir(&mut disk, apps).unwrap();
        assert!(names.iter().any(|(n, _)| n == "hello"));
        assert!(vol.lookup(&mut disk, "/nope").is_err());
        let bulky_ino = vol.lookup(&mut disk, "/Apps/hello/bulky.bin").unwrap();
        let mut bulky_out = std::vec![0u8; 8192];
        let n = vol
            .read_file(&mut disk, bulky_ino, 0, &mut bulky_out)
            .unwrap();
        assert_eq!(n, 8192);
        assert!(bulky_out.iter().all(|byte| *byte == 0x5A));
        let n = vol
            .read_file(&mut disk, bulky_ino, 100, &mut bulky_out[..50])
            .unwrap();
        assert_eq!(n, 50);
    }

    #[test]
    fn writes_survive_btrfs_check_and_a_torn_commit() {
        let root = std::env::temp_dir().join("vlr-btrfs-write-root");
        let image = std::env::temp_dir().join("vlr-btrfs-write.img");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Users/user")).unwrap();
        std::fs::create_dir_all(root.join("AppData/vch")).unwrap();
        std::fs::write(root.join("Users/user/motd"), b"vlr-motd\n").unwrap();
        std::fs::write(&image, std::vec![0u8; 32 * 1024 * 1024]).unwrap();
        let status = Command::new("mkfs.btrfs")
            .args([
                "-q", "-f", "-m", "single", "-d", "single", "-r",
                root.to_str().unwrap(), "--shrink", image.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success());
        let mut disk = MemDisk(std::fs::read(&image).unwrap());
        let mut vol = mount(&mut disk).unwrap();
        apply(&mut vol, &mut disk, true, |tx, vol, disk| {
            tx.mkdir("/Users/user/box")?;
            tx.write_path(vol, disk, "/Users/user/box/kept", b"persist-ok", false)?;
            tx.write_path(vol, disk, "/Users/user/box/big", &std::vec![0x11u8; 3000], false)?;
            tx.write_path(vol, disk, "/AppData/vch/history", b"write /Users/user/kept\n", false)?;
            tx.rename("/Users/user/box/kept", "/Users/user/box/note")?;
            Ok(())
        })
        .unwrap();
        let note = vol.lookup(&mut disk, "/Users/user/box/note").unwrap();
        let mut buf = [0u8; 32];
        let n = vol.read_file(&mut disk, note, 0, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"persist-ok");
        let big = vol.lookup(&mut disk, "/Users/user/box/big").unwrap();
        let mut big_buf = std::vec![0u8; 3000];
        let n = vol.read_file(&mut disk, big, 0, &mut big_buf).unwrap();
        assert_eq!(n, 3000);
        assert!(big_buf.iter().all(|byte| *byte == 0x11));
        assert!(vol.lookup(&mut disk, "/Users/user/box/kept").is_err());
        apply(&mut vol, &mut disk, true, |tx, _, _| tx.unlink("/Users/user/motd")).unwrap();
        assert!(vol.lookup(&mut disk, "/Users/user/motd").is_err());

        let torn = disk.0.clone();
        let mut torn_disk = MemDisk(torn);
        let mut torn_vol = mount(&mut torn_disk).unwrap();
        apply(&mut torn_vol, &mut torn_disk, false, |tx, vol, disk| {
            tx.write_path(vol, disk, "/Users/user/box/ghost", b"nope", false)
        })
        .unwrap();
        let mut reread = MemDisk(torn_disk.0.clone());
        let reread_vol = mount(&mut reread).unwrap();
        assert!(reread_vol.lookup(&mut reread, "/Users/user/box/ghost").is_err());
        assert!(reread_vol.lookup(&mut reread, "/Users/user/box/note").is_ok());

        std::fs::write(&image, &disk.0).unwrap();
        let check = Command::new("btrfs")
            .args(["check", "--readonly", image.to_str().unwrap()])
            .status()
            .expect("btrfs check");
        assert!(check.success(), "btrfs check rejected the written image");
    }
}
