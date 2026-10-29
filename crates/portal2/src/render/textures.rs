//! VTF textures to GPU images, with a cache keyed by path and colour space.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension};
use source_formats::vtf::{self, Vtf, VtfFormat};

use portal2_core::vfs::GameFs;

#[derive(Clone, Debug)]
pub struct TextureInfo {
    pub handle: Handle<Image>,
    pub width: u32,
    pub height: u32,
    pub flags: u32,
    pub has_alpha: bool,
    pub frames: u32,
    pub reflectivity: Vec3,
}

#[derive(Resource, Default)]
pub struct TextureCache {
    map: HashMap<(String, bool), Option<TextureInfo>>,
    missing: Option<Handle<Image>>,
}

impl TextureCache {
    /// Loads `materials/<name>.vtf` (any form accepted by `Vmt::texture_path`).
    pub fn load(&mut self, fs: &GameFs, images: &mut Assets<Image>, path: &str, srgb: bool) -> Option<TextureInfo> {
        let key = (path.to_owned(), srgb);
        if let Some(cached) = self.map.get(&key) {
            return cached.clone();
        }
        let info = fs.read(path).and_then(|bytes| match Vtf::parse(&bytes) {
            Ok(v) => Some(v),
            Err(e) => {
                warn!("bad texture {path}: {e}");
                None
            }
        });
        let info = info.map(|vtf| {
            let image = vtf_to_image(&vtf, srgb, 0);
            TextureInfo {
                handle: images.add(image),
                width: vtf.width,
                height: vtf.height,
                flags: vtf.flags,
                has_alpha: vtf.format.has_alpha()
                    && (vtf.flags & (vtf::flags::ONEBITALPHA | vtf::flags::EIGHTBITALPHA) != 0 || vtf.format != VtfFormat::Dxt5),
                frames: vtf.frames,
                reflectivity: Vec3::from(vtf.reflectivity),
            }
        });
        if info.is_none() {
            debug!("missing texture {path}");
        }
        self.map.insert(key, info.clone());
        info
    }

    /// One animation frame of a texture as its own image (texture toggles).
    pub fn load_frame(&mut self, fs: &GameFs, images: &mut Assets<Image>, path: &str, srgb: bool, frame: u32) -> Option<Handle<Image>> {
        if frame == 0 {
            return self.load(fs, images, path, srgb).map(|t| t.handle);
        }
        let key = (format!("{path}#{frame}"), srgb);
        if let Some(cached) = self.map.get(&key) {
            return cached.as_ref().map(|t| t.handle.clone());
        }
        let vtf = fs.read(path).and_then(|bytes| Vtf::parse(&bytes).ok())?;
        let image = vtf_to_image(&vtf, srgb, frame);
        let info = TextureInfo {
            handle: images.add(image),
            width: vtf.width,
            height: vtf.height,
            flags: vtf.flags,
            has_alpha: vtf.format.has_alpha(),
            frames: vtf.frames,
            reflectivity: Vec3::from(vtf.reflectivity),
        };
        let handle = info.handle.clone();
        self.map.insert(key, Some(info));
        Some(handle)
    }

    /// Generic magenta/black checkerboard used for anything that fails to load.
    pub fn missing(&mut self, images: &mut Assets<Image>) -> Handle<Image> {
        self.missing.get_or_insert_with(|| images.add(checkerboard())).clone()
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }
}

pub fn checkerboard() -> Image {
    let size = 32u32;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let on = ((x / 16) + (y / 16)) % 2 == 0;
            data.extend_from_slice(if on { &[255, 0, 255, 255] } else { &[0, 0, 0, 255] });
        }
    }
    let mut img = Image::new(
        Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::nearest();
    img
}

fn gpu_format(format: VtfFormat, srgb: bool, w: u32, h: u32) -> Option<TextureFormat> {
    let block_ok = w % 4 == 0 && h % 4 == 0;
    Some(match format {
        VtfFormat::Dxt1 | VtfFormat::Dxt1OneBitAlpha if block_ok => {
            if srgb {
                TextureFormat::Bc1RgbaUnormSrgb
            } else {
                TextureFormat::Bc1RgbaUnorm
            }
        }
        VtfFormat::Dxt3 if block_ok => {
            if srgb {
                TextureFormat::Bc2RgbaUnormSrgb
            } else {
                TextureFormat::Bc2RgbaUnorm
            }
        }
        VtfFormat::Dxt5 if block_ok => {
            if srgb {
                TextureFormat::Bc3RgbaUnormSrgb
            } else {
                TextureFormat::Bc3RgbaUnorm
            }
        }
        VtfFormat::Bgra8888 => {
            if srgb {
                TextureFormat::Bgra8UnormSrgb
            } else {
                TextureFormat::Bgra8Unorm
            }
        }
        VtfFormat::Rgba8888 => {
            if srgb {
                TextureFormat::Rgba8UnormSrgb
            } else {
                TextureFormat::Rgba8Unorm
            }
        }
        VtfFormat::Rgba16161616F => TextureFormat::Rgba16Float,
        _ => return None,
    })
}

/// Builds a Bevy image from a VTF frame. Block-compressed and 32-bit formats
/// upload directly; everything else is expanded to RGBA8.
pub fn vtf_to_image(vtf: &Vtf, srgb: bool, frame: u32) -> Image {
    let faces = if vtf.is_cubemap() { 6 } else { 1 };
    let frame = frame.min(vtf.frames.saturating_sub(1));
    let mut mips = vtf.mip_count;
    // Keep the chain consistent with the base size (some files store fewer).
    let max_mips = 32 - vtf.width.max(vtf.height).leading_zeros();
    mips = mips.min(max_mips).max(1);

    let direct = gpu_format(vtf.format, srgb, vtf.width, vtf.height);
    let format = direct.unwrap_or(if srgb { TextureFormat::Rgba8UnormSrgb } else { TextureFormat::Rgba8Unorm });
    let mut data = Vec::new();
    for face in 0..faces {
        for mip in 0..mips {
            if direct.is_some() {
                let (w, h, _) = vtf.mip_dims(mip);
                let img = vtf.image(mip, frame, face);
                // Only the first depth slice of volume textures is used.
                let slice = vtf.format.mip_size(w, h, 1);
                data.extend_from_slice(&img[..slice.min(img.len())]);
            } else {
                let (w, h, _) = vtf.mip_dims(mip);
                let rgba = vtf.decode_rgba8(mip, frame, face);
                data.extend_from_slice(&rgba[..(w * h * 4) as usize]);
            }
        }
    }

    let mut image = Image::new_uninit(
        Extent3d { width: vtf.width, height: vtf.height, depth_or_array_layers: faces },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = mips;
    if faces == 6 {
        image.texture_view_descriptor = Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::Cube), ..default() });
    }

    let clamp_s = vtf.flags & vtf::flags::CLAMPS != 0 || faces == 6;
    let clamp_t = vtf.flags & vtf::flags::CLAMPT != 0 || faces == 6;
    let point = vtf.flags & vtf::flags::POINTSAMPLE != 0;
    let filter = if point { ImageFilterMode::Nearest } else { ImageFilterMode::Linear };
    let addr = |clamp: bool| if clamp { ImageAddressMode::ClampToEdge } else { ImageAddressMode::Repeat };
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: addr(clamp_s),
        address_mode_v: addr(clamp_t),
        address_mode_w: addr(clamp_s),
        mag_filter: filter,
        min_filter: filter,
        mipmap_filter: if point { ImageFilterMode::Nearest } else { ImageFilterMode::Linear },
        anisotropy_clamp: if point || mips == 1 { 1 } else { 8 },
        ..default()
    });
    image
}
