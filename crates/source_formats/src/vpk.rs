//! Valve Pak (VPK v1/v2) archive reader.

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::reader::Reader;
use crate::text::normalize_path;
use crate::{Error, Result};

const VPK_SIGNATURE: u32 = 0x55AA_1234;
const DIR_ARCHIVE: u16 = 0x7FFF;

#[derive(Clone, Debug)]
pub struct VpkEntry {
    pub crc: u32,
    pub archive_index: u16,
    pub offset: u32,
    pub length: u32,
    preload_start: u32,
    preload_len: u16,
}

impl VpkEntry {
    pub fn size(&self) -> usize {
        self.preload_len as usize + self.length as usize
    }
}

pub struct Vpk {
    dir_path: PathBuf,
    /// Base name without the `_dir.vpk` suffix, used to locate `_NNN.vpk` archives.
    archive_stem: String,
    /// Offset of the embedded data section inside the dir file.
    data_offset: u64,
    dir_data: Vec<u8>,
    entries: HashMap<String, VpkEntry>,
    archives: Vec<OnceLock<Option<File>>>,
}

impl Vpk {
    pub fn open(dir_path: impl AsRef<Path>) -> Result<Self> {
        let dir_path = dir_path.as_ref().to_path_buf();
        let data = std::fs::read(&dir_path)?;
        let mut r = Reader::new(&data);
        let sig = r.u32()?;
        if sig != VPK_SIGNATURE {
            return Err(Error::Magic { expected: "VPK", found: data[..4].to_vec() });
        }
        let version = r.u32()?;
        let tree_size = r.u32()? as usize;
        let header_size = match version {
            1 => 12,
            2 => {
                r.skip(16)?;
                28
            }
            v => return Err(Error::Version(v as i64)),
        };

        let mut entries = HashMap::new();
        let mut max_archive = 0u16;
        let tree_end = header_size + tree_size;
        loop {
            let ext = r.cstr()?;
            if ext.is_empty() {
                break;
            }
            loop {
                let dir = r.cstr()?;
                if dir.is_empty() {
                    break;
                }
                loop {
                    let name = r.cstr()?;
                    if name.is_empty() {
                        break;
                    }
                    let crc = r.u32()?;
                    let preload_len = r.u16()?;
                    let archive_index = r.u16()?;
                    let offset = r.u32()?;
                    let length = r.u32()?;
                    let terminator = r.u16()?;
                    if terminator != 0xFFFF {
                        return Err(Error::Invalid(format!("vpk entry terminator {terminator:#x}")));
                    }
                    let preload_start = r.pos() as u32;
                    r.skip(preload_len as usize)?;
                    if archive_index != DIR_ARCHIVE {
                        max_archive = max_archive.max(archive_index);
                    }
                    let mut full = String::new();
                    if dir != " " {
                        full.push_str(&dir);
                        full.push('/');
                    }
                    full.push_str(&name);
                    if ext != " " {
                        full.push('.');
                        full.push_str(&ext);
                    }
                    entries.insert(
                        normalize_path(&full),
                        VpkEntry { crc, archive_index, offset, length, preload_start, preload_len },
                    );
                }
            }
        }
        if r.pos() > tree_end {
            log::warn!("vpk tree overran declared size in {}", dir_path.display());
        }

        let file_name = dir_path.file_name().and_then(|s| s.to_str()).unwrap_or_default().to_owned();
        let archive_stem = file_name.strip_suffix("_dir.vpk").unwrap_or(&file_name).to_owned();
        let archives = (0..=max_archive as usize).map(|_| OnceLock::new()).collect();
        Ok(Self {
            dir_path,
            archive_stem,
            data_offset: tree_end as u64,
            dir_data: data,
            entries,
            archives,
        })
    }

    pub fn path(&self) -> &Path {
        &self.dir_path
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(&normalize_path(path))
    }

    pub fn entry(&self, path: &str) -> Option<&VpkEntry> {
        self.entries.get(&normalize_path(path))
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(|s| s.as_str())
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let key = normalize_path(path);
        let entry = self.entries.get(&key).ok_or_else(|| Error::NotFound(key.clone()))?;
        self.read_entry(entry)
    }

    pub fn read_entry(&self, entry: &VpkEntry) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(entry.size());
        let ps = entry.preload_start as usize;
        out.extend_from_slice(&self.dir_data[ps..ps + entry.preload_len as usize]);
        if entry.length == 0 {
            return Ok(out);
        }
        let start = out.len();
        out.resize(start + entry.length as usize, 0);
        if entry.archive_index == DIR_ARCHIVE {
            let off = (self.data_offset + entry.offset as u64) as usize;
            let src = self
                .dir_data
                .get(off..off + entry.length as usize)
                .ok_or(Error::Eof)?;
            out[start..].copy_from_slice(src);
        } else {
            let file = self.archive(entry.archive_index)?;
            read_exact_at(file, &mut out[start..], entry.offset as u64)?;
        }
        Ok(out)
    }

    fn archive(&self, index: u16) -> Result<&File> {
        let slot = self.archives.get(index as usize).ok_or(Error::Eof)?;
        let file = slot.get_or_init(|| {
            let name = format!("{}_{:03}.vpk", self.archive_stem, index);
            let path = self.dir_path.with_file_name(name);
            File::open(&path).ok()
        });
        file.as_ref().ok_or_else(|| Error::NotFound(format!("{}_{:03}.vpk", self.archive_stem, index)))
    }
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = file.seek_read(buf, offset)?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        buf = &mut buf[n..];
        offset += n as u64;
    }
    Ok(())
}

#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buf, offset)
}
