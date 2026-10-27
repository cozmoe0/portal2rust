//! Little-endian binary cursor used by all the binary format readers.

use crate::{Error, Result};

#[derive(Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn at(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        if self.remaining() < n {
            return Err(Error::Eof);
        }
        self.pos += n;
        Ok(())
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(Error::Eof);
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let b = self.bytes(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(b);
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.array()?))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.array()?))
    }

    pub fn vec3(&mut self) -> Result<glam::Vec3> {
        Ok(glam::Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }

    pub fn vec2(&mut self) -> Result<glam::Vec2> {
        Ok(glam::Vec2::new(self.f32()?, self.f32()?))
    }

    pub fn quat(&mut self) -> Result<glam::Quat> {
        Ok(glam::Quat::from_xyzw(self.f32()?, self.f32()?, self.f32()?, self.f32()?))
    }

    /// Fixed-size, NUL padded string field.
    pub fn fixed_str(&mut self, n: usize) -> Result<String> {
        let b = self.bytes(n)?;
        Ok(str_from_nul(b))
    }

    /// NUL terminated string starting at the cursor.
    pub fn cstr(&mut self) -> Result<String> {
        let rest = &self.data[self.pos.min(self.data.len())..];
        let end = rest.iter().position(|&c| c == 0).ok_or(Error::Eof)?;
        let s = String::from_utf8_lossy(&rest[..end]).into_owned();
        self.pos += end + 1;
        Ok(s)
    }
}

/// Reads a NUL-terminated string at an absolute offset without moving any cursor.
pub fn cstr_at(data: &[u8], offset: usize) -> String {
    if offset >= data.len() {
        return String::new();
    }
    str_from_nul(&data[offset..])
}

pub fn str_from_nul(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// Reads `count` fixed-size records with a per-record parser.
pub fn read_records<'a, T>(
    data: &'a [u8],
    stride: usize,
    mut f: impl FnMut(&mut Reader<'a>) -> Result<T>,
) -> Result<Vec<T>> {
    if stride == 0 {
        return Ok(Vec::new());
    }
    let count = data.len() / stride;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let mut r = Reader::at(data, i * stride);
        out.push(f(&mut r)?);
    }
    Ok(out)
}
