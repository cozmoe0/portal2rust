//! Minimal ZIP reader for BSP pakfile lumps (stored and Valve-LZMA entries).

use std::collections::HashMap;

use crate::reader::Reader;
use crate::text::normalize_path;
use crate::{Error, Result};

#[derive(Clone, Debug)]
struct ZipEntry {
    method: u16,
    compressed_size: u32,
    uncompressed_size: u32,
    local_header_offset: u32,
}

#[derive(Default)]
pub struct ZipArchive {
    data: Vec<u8>,
    entries: HashMap<String, ZipEntry>,
}

impl ZipArchive {
    pub fn new(data: Vec<u8>) -> Result<Self> {
        if data.len() < 22 {
            return Ok(Self::default());
        }
        // Find the end-of-central-directory record.
        let min = data.len().saturating_sub(22 + 0xFFFF);
        let mut eocd = None;
        let mut i = data.len() - 22;
        loop {
            if data[i..i + 4] == [0x50, 0x4B, 0x05, 0x06] {
                eocd = Some(i);
                break;
            }
            if i == min {
                break;
            }
            i -= 1;
        }
        let eocd = eocd.ok_or_else(|| Error::Invalid("zip: no end of central directory".into()))?;
        let mut r = Reader::at(&data, eocd + 10);
        let total = r.u16()? as usize;
        let _cd_size = r.u32()?;
        let cd_offset = r.u32()? as usize;

        let mut entries = HashMap::with_capacity(total);
        let mut r = Reader::at(&data, cd_offset);
        for _ in 0..total {
            if r.u32()? != 0x0201_4B50 {
                return Err(Error::Invalid("zip: bad central directory entry".into()));
            }
            r.skip(6)?;
            let method = r.u16()?;
            r.skip(8)?;
            let compressed_size = r.u32()?;
            let uncompressed_size = r.u32()?;
            let name_len = r.u16()? as usize;
            let extra_len = r.u16()? as usize;
            let comment_len = r.u16()? as usize;
            r.skip(8)?;
            let local_header_offset = r.u32()?;
            let name = String::from_utf8_lossy(r.bytes(name_len)?).into_owned();
            r.skip(extra_len + comment_len)?;
            entries.insert(
                normalize_path(&name),
                ZipEntry { method, compressed_size, uncompressed_size, local_header_offset },
            );
        }
        Ok(Self { data, entries })
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

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(|s| s.as_str())
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let key = normalize_path(path);
        let e = self.entries.get(&key).ok_or_else(|| Error::NotFound(key.clone()))?;
        let mut r = Reader::at(&self.data, e.local_header_offset as usize);
        if r.u32()? != 0x0403_4B50 {
            return Err(Error::Invalid("zip: bad local header".into()));
        }
        r.skip(22)?;
        let name_len = r.u16()? as usize;
        let extra_len = r.u16()? as usize;
        r.skip(name_len + extra_len)?;
        let body = r.bytes(e.compressed_size as usize)?;
        match e.method {
            0 => Ok(body.to_vec()),
            14 => decode_valve_lzma(body, e.uncompressed_size as usize),
            m => Err(Error::Invalid(format!("zip: unsupported compression method {m} for {key}"))),
        }
    }
}

/// ZIP method 14 as written by Valve tools: 2-byte version, 2-byte props size,
/// 5 bytes of LZMA properties, then the raw stream without a size field.
fn decode_valve_lzma(body: &[u8], size: usize) -> Result<Vec<u8>> {
    if body.len() < 9 {
        return Err(Error::Eof);
    }
    let props = &body[4..9];
    let mut stream = Vec::with_capacity(13 + body.len());
    stream.extend_from_slice(props);
    stream.extend_from_slice(&(size as u64).to_le_bytes());
    stream.extend_from_slice(&body[9..]);
    let mut out = Vec::with_capacity(size);
    lzma_rs::lzma_decompress(&mut std::io::Cursor::new(stream), &mut out)
        .map_err(|e| Error::Invalid(format!("lzma: {e:?}")))?;
    Ok(out)
}

/// Decodes Valve's standalone LZMA container ("LZMA" magic, used by compiled
/// scenes and compressed lumps).
pub fn decode_valve_lzma_header(data: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(data);
    let magic = r.bytes(4)?;
    if magic != b"LZMA" {
        return Err(Error::Magic { expected: "LZMA", found: magic.to_vec() });
    }
    let actual_size = r.u32()? as usize;
    let _lzma_size = r.u32()?;
    let props = r.bytes(5)?;
    let mut stream = Vec::with_capacity(13 + r.remaining());
    stream.extend_from_slice(props);
    stream.extend_from_slice(&(actual_size as u64).to_le_bytes());
    stream.extend_from_slice(r.bytes(r.remaining())?);
    let mut out = Vec::with_capacity(actual_size);
    lzma_rs::lzma_decompress(&mut std::io::Cursor::new(stream), &mut out)
        .map_err(|e| Error::Invalid(format!("lzma: {e:?}")))?;
    Ok(out)
}
