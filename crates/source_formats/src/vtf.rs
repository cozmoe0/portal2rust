//! Valve Texture Format (VTF 7.0 - 7.5) reader and pixel decoders.

use crate::reader::Reader;
use crate::{Error, Result};

pub mod flags {
    pub const POINTSAMPLE: u32 = 0x0000_0001;
    pub const TRILINEAR: u32 = 0x0000_0002;
    pub const CLAMPS: u32 = 0x0000_0004;
    pub const CLAMPT: u32 = 0x0000_0008;
    pub const ANISOTROPIC: u32 = 0x0000_0010;
    pub const HINT_DXT5: u32 = 0x0000_0020;
    pub const SRGB: u32 = 0x0000_0040;
    pub const NORMAL: u32 = 0x0000_0080;
    pub const NOMIP: u32 = 0x0000_0100;
    pub const NOLOD: u32 = 0x0000_0200;
    pub const ALL_MIPS: u32 = 0x0000_0400;
    pub const PROCEDURAL: u32 = 0x0000_0800;
    pub const ONEBITALPHA: u32 = 0x0000_1000;
    pub const EIGHTBITALPHA: u32 = 0x0000_2000;
    pub const ENVMAP: u32 = 0x0000_4000;
    pub const RENDERTARGET: u32 = 0x0000_8000;
    pub const DEPTHRENDERTARGET: u32 = 0x0001_0000;
    pub const NODEBUGOVERRIDE: u32 = 0x0002_0000;
    pub const SINGLECOPY: u32 = 0x0004_0000;
    pub const NODEPTHBUFFER: u32 = 0x0080_0000;
    pub const CLAMPU: u32 = 0x0200_0000;
    pub const VERTEXTEXTURE: u32 = 0x0400_0000;
    pub const SSBUMP: u32 = 0x0800_0000;
    pub const BORDER: u32 = 0x2000_0000;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VtfFormat {
    Rgba8888,
    Abgr8888,
    Rgb888,
    Bgr888,
    Rgb565,
    I8,
    Ia88,
    P8,
    A8,
    Rgb888Bluescreen,
    Bgr888Bluescreen,
    Argb8888,
    Bgra8888,
    Dxt1,
    Dxt3,
    Dxt5,
    Bgrx8888,
    Bgr565,
    Bgrx5551,
    Bgra4444,
    Dxt1OneBitAlpha,
    Bgra5551,
    Uv88,
    Uvwq8888,
    Rgba16161616F,
    Rgba16161616,
    Uvlx8888,
    R32F,
    Rgb323232F,
    Rgba32323232F,
    NvDst16,
    NvDst24,
    NvIntz,
    NvRawz,
    AtiDst16,
    AtiDst24,
    NvNull,
    Ati2n,
    Ati1n,
    None,
    Unknown(i32),
}

impl VtfFormat {
    pub fn from_i32(v: i32) -> Self {
        use VtfFormat::*;
        const TABLE: [VtfFormat; 39] = [
            Rgba8888, Abgr8888, Rgb888, Bgr888, Rgb565, I8, Ia88, P8, A8, Rgb888Bluescreen, Bgr888Bluescreen,
            Argb8888, Bgra8888, Dxt1, Dxt3, Dxt5, Bgrx8888, Bgr565, Bgrx5551, Bgra4444, Dxt1OneBitAlpha,
            Bgra5551, Uv88, Uvwq8888, Rgba16161616F, Rgba16161616, Uvlx8888, R32F, Rgb323232F, Rgba32323232F,
            NvDst16, NvDst24, NvIntz, NvRawz, AtiDst16, AtiDst24, NvNull, Ati2n, Ati1n,
        ];
        if v == -1 {
            return None;
        }
        TABLE.get(v as usize).copied().unwrap_or(Unknown(v))
    }

    /// (block width, block bytes). Uncompressed formats use a block width of 1.
    pub fn block_info(self) -> (u32, u32) {
        use VtfFormat::*;
        match self {
            Dxt1 | Dxt1OneBitAlpha | Ati1n => (4, 8),
            Dxt3 | Dxt5 | Ati2n => (4, 16),
            Rgba8888 | Abgr8888 | Argb8888 | Bgra8888 | Bgrx8888 | Uvwq8888 | Uvlx8888 | R32F | NvDst24
            | NvIntz | NvRawz | AtiDst24 | NvNull => (1, 4),
            Rgb888 | Bgr888 | Rgb888Bluescreen | Bgr888Bluescreen => (1, 3),
            Rgb565 | Bgr565 | Bgrx5551 | Bgra4444 | Bgra5551 | Ia88 | Uv88 | NvDst16 | AtiDst16 => (1, 2),
            I8 | P8 | A8 => (1, 1),
            Rgba16161616F | Rgba16161616 => (1, 8),
            Rgb323232F => (1, 12),
            Rgba32323232F => (1, 16),
            None | Unknown(_) => (1, 0),
        }
    }

    pub fn is_compressed(self) -> bool {
        self.block_info().0 == 4
    }

    pub fn has_alpha(self) -> bool {
        use VtfFormat::*;
        matches!(
            self,
            Rgba8888 | Abgr8888 | Argb8888 | Bgra8888 | Dxt3 | Dxt5 | Dxt1OneBitAlpha | Bgra4444 | Bgra5551
                | Ia88 | A8 | Rgba16161616F | Rgba16161616 | Rgba32323232F
        )
    }

    pub fn mip_size(self, w: u32, h: u32, d: u32) -> usize {
        let (bw, bb) = self.block_info();
        if bw == 4 {
            (w.div_ceil(4).max(1) * h.div_ceil(4).max(1) * bb) as usize * d as usize
        } else {
            (w.max(1) * h.max(1) * d.max(1) * bb) as usize
        }
    }
}

/// Particle sheet (VTF resource `0x10`): animation sequences of rectangles
/// inside the texture, used by `SpriteCard` particle materials.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sheet {
    pub sequences: Vec<SheetSequence>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SheetSequence {
    /// Sequence number particles select it by (not necessarily its index).
    pub number: u32,
    /// Bit 0: hold the last frame instead of looping. Bits 1 and 2 mark a
    /// sequence packed for colour only or alpha only.
    pub flags: u32,
    /// Sum of the frame display times.
    pub total_time: f32,
    pub frames: Vec<SheetFrame>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SheetFrame {
    pub display_time: f32,
    /// `u0, v0, u1, v1` of up to four images (version 0 sheets have one).
    pub rects: Vec<[f32; 4]>,
}

impl SheetSequence {
    pub const CLAMP: u32 = 1;

    pub fn clamps(&self) -> bool {
        self.flags & Self::CLAMP != 0
    }
}

impl Sheet {
    /// Parses the resource body: version, sequence count, then per sequence
    /// its number, flags, frame count and total time, and per frame its
    /// display time and one (version 0) or four (version 1) rectangles.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let version = r.u32()?;
        let count = r.u32()?;
        let images = if version == 0 { 1 } else { 4 };
        let mut sequences = Vec::with_capacity(count.min(1024) as usize);
        for _ in 0..count {
            let number = r.u32()?;
            let flags = r.u32()?;
            let frame_count = r.u32()?;
            let total_time = r.f32()?;
            let mut frames = Vec::with_capacity(frame_count.min(4096) as usize);
            for _ in 0..frame_count {
                let display_time = r.f32()?;
                let mut rects = Vec::with_capacity(images);
                for _ in 0..images {
                    rects.push([r.f32()?, r.f32()?, r.f32()?, r.f32()?]);
                }
                frames.push(SheetFrame { display_time, rects });
            }
            sequences.push(SheetSequence { number, flags, total_time, frames });
        }
        Ok(Self { sequences })
    }

    pub fn sequence(&self, number: u32) -> Option<&SheetSequence> {
        self.sequences.iter().find(|s| s.number == number)
    }
}

#[derive(Clone, Debug)]
pub struct Vtf {
    pub version: (u32, u32),
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub flags: u32,
    pub frames: u32,
    pub first_frame: u32,
    pub reflectivity: [f32; 3],
    pub bumpmap_scale: f32,
    pub format: VtfFormat,
    pub mip_count: u32,
    pub faces: u32,
    /// Particle sheet, when the file has one.
    pub sheet: Option<Sheet>,
    /// High resolution image data, VTF order (smallest mip first).
    data: Vec<u8>,
    /// Offset into `data` of each mip level (index 0 = largest).
    mip_offsets: Vec<usize>,
}

impl Vtf {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let magic = r.bytes(4)?;
        if magic != b"VTF\0" {
            return Err(Error::Magic { expected: "VTF", found: magic.to_vec() });
        }
        let major = r.u32()?;
        let minor = r.u32()?;
        if major != 7 || minor > 5 {
            return Err(Error::Version((major * 10 + minor) as i64));
        }
        let header_size = r.u32()? as usize;
        let width = r.u16()? as u32;
        let height = r.u16()? as u32;
        let flags = r.u32()?;
        let frames = r.u16()?.max(1) as u32;
        let first_frame = r.u16()? as u32;
        r.skip(4)?;
        let reflectivity = [r.f32()?, r.f32()?, r.f32()?];
        r.skip(4)?;
        let bumpmap_scale = r.f32()?;
        let format = VtfFormat::from_i32(r.i32()?);
        let mip_count = r.u8()?.max(1) as u32;
        let lowres_format = VtfFormat::from_i32(r.i32()?);
        let lowres_w = r.u8()? as u32;
        let lowres_h = r.u8()? as u32;
        let depth = if minor >= 2 { r.u16()?.max(1) as u32 } else { 1 };

        let faces = if flags & flags::ENVMAP != 0 {
            if minor < 5 && first_frame != 0xFFFF { 7 } else { 6 }
        } else {
            1
        };

        let lowres_size =
            if lowres_format == VtfFormat::None || lowres_w == 0 { 0 } else { lowres_format.mip_size(lowres_w, lowres_h, 1) };

        let mut hires_offset = header_size + lowres_size;
        let mut sheet = None;
        if minor >= 3 {
            let mut rr = Reader::at(bytes, 68);
            let num_resources = rr.u32()?;
            rr.skip(8)?;
            for _ in 0..num_resources {
                let tag = rr.bytes(3)?;
                let rflags = rr.u8()?;
                let offset = rr.u32()? as usize;
                if tag == [0x30, 0, 0] {
                    hires_offset = offset;
                } else if tag == [0x10, 0, 0] && rflags & 2 == 0 {
                    // The offset points at the size of the block, then its data.
                    let mut sr = Reader::at(bytes, offset);
                    let size = sr.u32()? as usize;
                    let body = bytes
                        .get(offset + 4..offset + 4 + size)
                        .ok_or_else(|| Error::Invalid("vtf: sheet truncated".into()))?;
                    sheet = Some(Sheet::parse(body)?);
                }
            }
        }

        // Compute per-mip offsets, smallest mip stored first.
        let mut mip_sizes = Vec::with_capacity(mip_count as usize);
        for mip in 0..mip_count {
            let (w, h, d) = mip_dims(width, height, depth, mip);
            mip_sizes.push(format.mip_size(w, h, d) * (frames * faces) as usize);
        }
        let mut mip_offsets = vec![0usize; mip_count as usize];
        let mut off = 0usize;
        for mip in (0..mip_count as usize).rev() {
            mip_offsets[mip] = off;
            off += mip_sizes[mip];
        }
        let end = hires_offset + off;
        if end > bytes.len() {
            return Err(Error::Invalid(format!(
                "vtf: image data truncated ({} > {}) fmt {:?} {}x{} mips {}",
                end,
                bytes.len(),
                format,
                width,
                height,
                mip_count
            )));
        }
        Ok(Self {
            version: (major, minor),
            width,
            height,
            depth,
            flags,
            frames,
            first_frame,
            reflectivity,
            bumpmap_scale,
            format,
            mip_count,
            faces,
            sheet,
            data: bytes[hires_offset..end].to_vec(),
            mip_offsets,
        })
    }

    pub fn is_cubemap(&self) -> bool {
        self.flags & flags::ENVMAP != 0
    }

    pub fn mip_dims(&self, mip: u32) -> (u32, u32, u32) {
        mip_dims(self.width, self.height, self.depth, mip)
    }

    /// Raw bytes of one image (frame/face) at a mip level, all depth slices.
    pub fn image(&self, mip: u32, frame: u32, face: u32) -> &[u8] {
        let (w, h, d) = self.mip_dims(mip);
        let size = self.format.mip_size(w, h, d);
        let index = (frame * self.faces + face) as usize;
        let start = self.mip_offsets[mip as usize] + index * size;
        &self.data[start..start + size]
    }

    /// Decodes one image to RGBA8 (straight alpha).
    pub fn decode_rgba8(&self, mip: u32, frame: u32, face: u32) -> Vec<u8> {
        let (w, h, d) = self.mip_dims(mip);
        decode_to_rgba8(self.format, self.image(mip, frame, face), w, h * d)
    }

    /// Decodes one image to linear RGBA f32 (for HDR formats; LDR formats are
    /// returned as 0..1 values without sRGB conversion).
    pub fn decode_rgba_f32(&self, mip: u32, frame: u32, face: u32) -> Vec<[f32; 4]> {
        let (w, h, d) = self.mip_dims(mip);
        let src = self.image(mip, frame, face);
        let n = (w * h * d) as usize;
        match self.format {
            VtfFormat::Rgba16161616F => (0..n)
                .map(|i| {
                    let p = &src[i * 8..i * 8 + 8];
                    [
                        f16_to_f32(u16::from_le_bytes([p[0], p[1]])),
                        f16_to_f32(u16::from_le_bytes([p[2], p[3]])),
                        f16_to_f32(u16::from_le_bytes([p[4], p[5]])),
                        f16_to_f32(u16::from_le_bytes([p[6], p[7]])),
                    ]
                })
                .collect(),
            VtfFormat::Rgba16161616 => (0..n)
                .map(|i| {
                    let p = &src[i * 8..i * 8 + 8];
                    let c = |o: usize| u16::from_le_bytes([p[o], p[o + 1]]) as f32 / 65535.0;
                    [c(0), c(2), c(4), c(6)]
                })
                .collect(),
            VtfFormat::Rgba32323232F => (0..n)
                .map(|i| {
                    let p = &src[i * 16..i * 16 + 16];
                    let c = |o: usize| f32::from_le_bytes([p[o], p[o + 1], p[o + 2], p[o + 3]]);
                    [c(0), c(4), c(8), c(12)]
                })
                .collect(),
            VtfFormat::Rgb323232F => (0..n)
                .map(|i| {
                    let p = &src[i * 12..i * 12 + 12];
                    let c = |o: usize| f32::from_le_bytes([p[o], p[o + 1], p[o + 2], p[o + 3]]);
                    [c(0), c(4), c(8), 1.0]
                })
                .collect(),
            _ => decode_to_rgba8(self.format, src, w, h * d)
                .chunks_exact(4)
                .map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, p[3] as f32 / 255.0])
                .collect(),
        }
    }
}

pub fn mip_dims(w: u32, h: u32, d: u32, mip: u32) -> (u32, u32, u32) {
    ((w >> mip).max(1), (h >> mip).max(1), (d >> mip).max(1))
}

pub fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1F) as u32;
    let mant = (h & 0x3FF) as u32;
    let bits = if exp == 0 {
        if mant == 0 {
            sign << 31
        } else {
            // Subnormal: normalise.
            let mut e = 127 - 15 + 1;
            let mut m = mant;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            (sign << 31) | ((e as u32) << 23) | ((m & 0x3FF) << 13)
        }
    } else if exp == 31 {
        (sign << 31) | 0x7F80_0000 | (mant << 13)
    } else {
        (sign << 31) | ((exp + 127 - 15) << 23) | (mant << 13)
    };
    f32::from_bits(bits)
}

fn rgb565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 0x1F) as u32;
    let g = ((c >> 5) & 0x3F) as u32;
    let b = (c & 0x1F) as u32;
    [((r * 527 + 23) >> 6) as u8, ((g * 259 + 33) >> 6) as u8, ((b * 527 + 23) >> 6) as u8]
}

fn decode_bc1_block(block: &[u8], out: &mut [[u8; 4]; 16], allow_alpha: bool) {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let p0 = rgb565(c0);
    let p1 = rgb565(c1);
    let mut pal = [[0u8; 4]; 4];
    pal[0] = [p0[0], p0[1], p0[2], 255];
    pal[1] = [p1[0], p1[1], p1[2], 255];
    if c0 > c1 || !allow_alpha {
        for i in 0..3 {
            pal[2][i] = ((2 * p0[i] as u16 + p1[i] as u16) / 3) as u8;
            pal[3][i] = ((p0[i] as u16 + 2 * p1[i] as u16) / 3) as u8;
        }
        pal[2][3] = 255;
        pal[3][3] = 255;
    } else {
        for i in 0..3 {
            pal[2][i] = ((p0[i] as u16 + p1[i] as u16) / 2) as u8;
        }
        pal[2][3] = 255;
        pal[3] = [0, 0, 0, 0];
    }
    let bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    for (i, px) in out.iter_mut().enumerate() {
        *px = pal[((bits >> (2 * i)) & 3) as usize];
    }
}

fn decode_bc4_channel(block: &[u8], out: &mut [u8; 16]) {
    let a0 = block[0] as u32;
    let a1 = block[1] as u32;
    let mut pal = [0u8; 8];
    pal[0] = a0 as u8;
    pal[1] = a1 as u8;
    if a0 > a1 {
        for i in 1..7 {
            pal[i + 1] = (((7 - i as u32) * a0 + i as u32 * a1) / 7) as u8;
        }
    } else {
        for i in 1..5 {
            pal[i + 1] = (((5 - i as u32) * a0 + i as u32 * a1) / 5) as u8;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let mut bits = 0u64;
    for i in 0..6 {
        bits |= (block[2 + i] as u64) << (8 * i);
    }
    for (i, v) in out.iter_mut().enumerate() {
        *v = pal[((bits >> (3 * i)) & 7) as usize];
    }
}

/// Decodes any supported VTF pixel format into RGBA8.
pub fn decode_to_rgba8(format: VtfFormat, src: &[u8], w: u32, h: u32) -> Vec<u8> {
    use VtfFormat::*;
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 4];
    let px = |out: &mut Vec<u8>, i: usize, c: [u8; 4]| out[i * 4..i * 4 + 4].copy_from_slice(&c);
    match format {
        Dxt1 | Dxt1OneBitAlpha | Dxt3 | Dxt5 | Ati1n | Ati2n => {
            let (_, bb) = format.block_info();
            let bw = w.div_ceil(4).max(1);
            let bh = h.div_ceil(4).max(1);
            for by in 0..bh {
                for bx in 0..bw {
                    let off = (by * bw + bx) * bb as usize;
                    let Some(block) = src.get(off..off + bb as usize) else { continue };
                    let mut texels = [[0u8; 4]; 16];
                    match format {
                        Dxt1 | Dxt1OneBitAlpha => decode_bc1_block(block, &mut texels, true),
                        Dxt3 => {
                            decode_bc1_block(&block[8..], &mut texels, false);
                            for i in 0..16 {
                                let nib = (block[i / 2] >> ((i % 2) * 4)) & 0xF;
                                texels[i][3] = nib * 17;
                            }
                        }
                        Dxt5 => {
                            decode_bc1_block(&block[8..], &mut texels, false);
                            let mut a = [0u8; 16];
                            decode_bc4_channel(&block[..8], &mut a);
                            for i in 0..16 {
                                texels[i][3] = a[i];
                            }
                        }
                        Ati1n => {
                            let mut a = [0u8; 16];
                            decode_bc4_channel(&block[..8], &mut a);
                            for i in 0..16 {
                                texels[i] = [a[i], a[i], a[i], 255];
                            }
                        }
                        Ati2n => {
                            let mut x = [0u8; 16];
                            let mut y = [0u8; 16];
                            decode_bc4_channel(&block[..8], &mut x);
                            decode_bc4_channel(&block[8..], &mut y);
                            for i in 0..16 {
                                let nx = x[i] as f32 / 127.5 - 1.0;
                                let ny = y[i] as f32 / 127.5 - 1.0;
                                let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
                                texels[i] = [x[i], y[i], ((nz * 0.5 + 0.5) * 255.0) as u8, 255];
                            }
                        }
                        _ => unreachable!(),
                    }
                    for ty in 0..4 {
                        for tx in 0..4 {
                            let x = bx * 4 + tx;
                            let y = by * 4 + ty;
                            if x < w && y < h {
                                px(&mut out, y * w + x, texels[ty * 4 + tx]);
                            }
                        }
                    }
                }
            }
        }
        _ => {
            let (_, bpp) = format.block_info();
            let bpp = bpp as usize;
            for i in 0..w * h {
                let Some(p) = src.get(i * bpp..i * bpp + bpp) else { break };
                let c = match format {
                    Rgba8888 => [p[0], p[1], p[2], p[3]],
                    Abgr8888 => [p[3], p[2], p[1], p[0]],
                    Argb8888 => [p[1], p[2], p[3], p[0]],
                    Bgra8888 => [p[2], p[1], p[0], p[3]],
                    Bgrx8888 => [p[2], p[1], p[0], 255],
                    Rgb888 => [p[0], p[1], p[2], 255],
                    Bgr888 => [p[2], p[1], p[0], 255],
                    Rgb888Bluescreen => {
                        if p[0] == 0 && p[1] == 0 && p[2] == 255 { [0, 0, 0, 0] } else { [p[0], p[1], p[2], 255] }
                    }
                    Bgr888Bluescreen => {
                        if p[0] == 255 && p[1] == 0 && p[2] == 0 { [0, 0, 0, 0] } else { [p[2], p[1], p[0], 255] }
                    }
                    Rgb565 => {
                        let c = rgb565(u16::from_le_bytes([p[0], p[1]]));
                        [c[0], c[1], c[2], 255]
                    }
                    Bgr565 => {
                        let c = rgb565(u16::from_le_bytes([p[0], p[1]]));
                        [c[2], c[1], c[0], 255]
                    }
                    Bgra4444 => {
                        let v = u16::from_le_bytes([p[0], p[1]]);
                        let n = |s: u16| (((v >> s) & 0xF) as u8) * 17;
                        [n(8), n(4), n(0), n(12)]
                    }
                    Bgra5551 | Bgrx5551 => {
                        let v = u16::from_le_bytes([p[0], p[1]]);
                        let n = |s: u16| ((((v >> s) & 0x1F) as u32 * 527 + 23) >> 6) as u8;
                        let a = if format == Bgrx5551 || v & 0x8000 != 0 { 255 } else { 0 };
                        [n(10), n(5), n(0), a]
                    }
                    I8 => [p[0], p[0], p[0], 255],
                    Ia88 => [p[0], p[0], p[0], p[1]],
                    A8 => [255, 255, 255, p[0]],
                    P8 => [p[0], p[0], p[0], 255],
                    Uv88 => [p[0], p[1], 255, 255],
                    Uvwq8888 | Uvlx8888 => [p[0], p[1], p[2], p[3]],
                    Rgba16161616F => {
                        let f = |o: usize| {
                            let v = f16_to_f32(u16::from_le_bytes([p[o], p[o + 1]]));
                            (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
                        };
                        [f(0), f(2), f(4), f(6)]
                    }
                    Rgba16161616 => [p[1], p[3], p[5], p[7]],
                    _ => [255, 0, 255, 255],
                };
                px(&mut out, i, c);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_f32(out: &mut Vec<u8>, v: f32) {
        out.extend_from_slice(&v.to_le_bytes());
    }

    #[test]
    fn parses_sheet_sequences() {
        let mut b = Vec::new();
        // Version 1 (four rectangles per frame), one sequence.
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&7u32.to_le_bytes()); // number
        b.extend_from_slice(&1u32.to_le_bytes()); // clamp
        b.extend_from_slice(&2u32.to_le_bytes()); // frames
        push_f32(&mut b, 3.0);
        for (time, u) in [(1.0, 0.0), (2.0, 0.5)] {
            push_f32(&mut b, time);
            for _ in 0..4 {
                for v in [u, 0.0, u + 0.5, 1.0] {
                    push_f32(&mut b, v);
                }
            }
        }
        let sheet = Sheet::parse(&b).unwrap();
        let s = sheet.sequence(7).unwrap();
        assert!(s.clamps());
        assert_eq!(s.total_time, 3.0);
        assert_eq!(s.frames.len(), 2);
        assert_eq!(s.frames[1].display_time, 2.0);
        assert_eq!(s.frames[1].rects[0], [0.5, 0.0, 1.0, 1.0]);
        assert_eq!(s.frames[1].rects.len(), 4);
        assert!(Sheet::parse(&b[..b.len() - 4]).is_err());
    }
}
