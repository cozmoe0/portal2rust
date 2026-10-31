//! Reflection probes for the overhaul: the map's own baked cube maps (the
//! `env_cubemap` captures vbsp packs into the map), resampled into one cube
//! array that every world and model material binds, with their origins in
//! the shared buffer. Wet patches and glossy materials the game draws
//! without a reflection pick the nearest probe, as the engine does for
//! models' `env_cubemap`, and blur it with their roughness through the
//! array's mips.
//!
//! The array has a fixed size, so loading a map only refills it.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor, TextureViewDimension,
};
use portal2_render::fx::{FX, MAX_PROBES, PROBE_SIZE, PROBES_OFFSET};
use source_formats::vtf::{Vtf, VtfFormat};

use super::material::FxBuffer;
use portal2_core::vfs::GameFs;
use portal2_world::CurrentMap;

/// Mips of each probe face (64 down to 1).
const MIPS: u32 = PROBE_SIZE.ilog2() + 1;
/// Where the probe count lives in the header (`reserved0.x`).
const PROBE_COUNT: usize = 24;

/// Bytes of one face with all its mips (RGBA, 16-bit floats).
fn face_bytes() -> usize {
    (0..MIPS).map(|m| ((PROBE_SIZE >> m) * (PROBE_SIZE >> m)) as usize * 8).sum()
}

/// The empty cube array the materials bind from startup.
pub fn probe_image() -> Image {
    probe_image_from(vec![0u8; face_bytes() * 6 * MAX_PROBES])
}

fn probe_image_from(data: Vec<u8>) -> Image {
    let mut image = Image::new_uninit(
        Extent3d { width: PROBE_SIZE, height: PROBE_SIZE, depth_or_array_layers: 6 * MAX_PROBES as u32 },
        TextureDimension::D2,
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = MIPS;
    image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST;
    image.texture_view_descriptor = Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::CubeArray), ..default() });
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        address_mode_w: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

pub struct FxReflectionsPlugin;

impl Plugin for FxReflectionsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, load_probes.after(portal2_core::sets::LoadMapSet));
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// One cube face as linear RGB at `PROBE_SIZE`, from the mip nearest that size.
fn face_pixels(vtf: &Vtf, face: u32, linear: bool) -> Vec<Vec3> {
    let mut mip = 0;
    while mip + 1 < vtf.mip_count && vtf.mip_dims(mip + 1).0 >= PROBE_SIZE {
        mip += 1;
    }
    let (w, h, _) = vtf.mip_dims(mip);
    let src: Vec<Vec3> = match vtf.format {
        VtfFormat::Rgba16161616F | VtfFormat::Rgba16161616 | VtfFormat::Rgba32323232F => {
            vtf.decode_rgba_f32(mip, 0, face).into_iter().map(|p| Vec3::new(p[0], p[1], p[2])).collect()
        }
        _ => vtf
            .decode_rgba8(mip, 0, face)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                let c = Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32) / 255.0;
                if linear { c } else { Vec3::new(srgb_to_linear(c.x), srgb_to_linear(c.y), srgb_to_linear(c.z)) }
            })
            .collect(),
    };
    let (w, h) = (w as usize, h as usize);
    if w == 0 || h == 0 || src.len() < w * h {
        return vec![Vec3::ZERO; (PROBE_SIZE * PROBE_SIZE) as usize];
    }
    let n = PROBE_SIZE as usize;
    let mut out = Vec::with_capacity(n * n);
    for y in 0..n {
        let fy = ((y as f32 + 0.5) * h as f32 / n as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        let y1 = (y0 + 1).min(h - 1);
        for x in 0..n {
            let fx = ((x as f32 + 0.5) * w as f32 / n as f32 - 0.5).clamp(0.0, (w - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let x1 = (x0 + 1).min(w - 1);
            let top = src[y0 * w + x0].lerp(src[y0 * w + x1], tx);
            let bottom = src[y1 * w + x0].lerp(src[y1 * w + x1], tx);
            out.push(top.lerp(bottom, ty));
        }
    }
    out
}

/// Whether a face is one flat, bright colour: a capture that went wrong
/// (`sp_a1_wakeup`'s probe in the breaker shaft reads 19.7 across its whole
/// top face), which would turn everything near it white. The game itself
/// only shows such a probe on the few materials that ask for a reflection.
fn saturated(face: &[Vec3]) -> bool {
    let lum = |c: &Vec3| c.dot(Vec3::new(0.2126, 0.7152, 0.0722));
    let (lo, hi) = face.iter().map(lum).fold((f32::MAX, f32::MIN), |(lo, hi), l| (lo.min(l), hi.max(l)));
    hi > 2.0 && hi - lo < hi * 0.01
}

/// A face and its box-filtered mips, as RGBA 16-bit floats.
fn face_with_mips(mut level: Vec<Vec3>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(face_bytes());
    let mut size = PROBE_SIZE as usize;
    loop {
        for c in &level {
            for v in [c.x, c.y, c.z, 1.0] {
                bytes.extend_from_slice(&half::f16::from_f32(v.max(0.0)).to_le_bytes());
            }
        }
        if size == 1 {
            break;
        }
        let half_size = size / 2;
        let mut next = Vec::with_capacity(half_size * half_size);
        for y in 0..half_size {
            for x in 0..half_size {
                let s = |dx: usize, dy: usize| level[(y * 2 + dy) * size + x * 2 + dx];
                next.push((s(0, 0) + s(1, 0) + s(0, 1) + s(1, 1)) * 0.25);
            }
        }
        level = next;
        size = half_size;
    }
    bytes
}

fn load_probes(
    loaded: Option<Res<portal2_core::LoadedMap>>,
    mut seen: Local<u64>,
    map: Option<Res<CurrentMap>>,
    fs: Res<GameFs>,
    mut images: ResMut<Assets<Image>>,
    mut buffer: ResMut<FxBuffer>,
) {
    // Every load changes the generation (MapLoaded messages are drained by
    // the entity spawner).
    let generation = loaded.map_or(0, |l| l.generation);
    if generation == *seen {
        return;
    }
    *seen = generation;
    if generation == 0 {
        return;
    }
    let (Some(map), Some(handles)) = (map, FX.get()) else { return };
    let face = face_bytes();
    let mut data = vec![0u8; face * 6 * MAX_PROBES];
    let mut count = 0;
    for c in map.bsp.cubemaps.iter().take(MAX_PROBES) {
        let base = format!("materials/maps/{}/c{}_{}_{}", map.name, c.origin[0], c.origin[1], c.origin[2]);
        // The HDR capture is linear; the other one is gamma encoded.
        let found = fs
            .read(&format!("{base}.hdr.vtf"))
            .and_then(|b| Vtf::parse(&b).ok())
            .map(|v| (v, true))
            .or_else(|| fs.read(&format!("{base}.vtf")).and_then(|b| Vtf::parse(&b).ok()).map(|v| (v, false)));
        let Some((vtf, linear)) = found.filter(|(v, _)| v.is_cubemap()) else { continue };
        let faces: Vec<Vec<Vec3>> = (0..6u32).map(|f| face_pixels(&vtf, f, linear)).collect();
        if faces.iter().any(|f| saturated(f)) {
            debug!("skipping reflection probe at {:?}: a face is one flat bright colour", c.origin);
            continue;
        }
        for (f, pixels) in faces.into_iter().enumerate() {
            let o = (count * 6 + f) * face;
            data[o..o + face].copy_from_slice(&face_with_mips(pixels));
        }
        let origin = Vec3::new(c.origin[0] as f32, c.origin[1] as f32, c.origin[2] as f32);
        let p = PROBES_OFFSET + count * 4;
        buffer.data[p..p + 4].copy_from_slice(&[origin.x, origin.y, origin.z, 1.0]);
        count += 1;
    }
    buffer.data[PROBE_COUNT] = count as f32;
    if let Some(mut image) = images.get_mut(&handles.probes) {
        *image = probe_image_from(data);
    }
    info!("{count} reflection probes for the visual overhaul");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_bright_face_is_saturated() {
        let n = (PROBE_SIZE * PROBE_SIZE) as usize;
        assert!(saturated(&vec![Vec3::splat(19.7); n]));
        // A dim flat face (a dark corner) and a bright varied one are fine.
        assert!(!saturated(&vec![Vec3::splat(0.02); n]));
        let mut varied = vec![Vec3::splat(0.1); n];
        varied[0] = Vec3::splat(19.7);
        assert!(!saturated(&varied));
    }

    #[test]
    fn mip_chain_layout() {
        let bytes = face_with_mips(vec![Vec3::ONE; (PROBE_SIZE * PROBE_SIZE) as usize]);
        assert_eq!(bytes.len(), face_bytes());
        // The smallest mip of a uniform face keeps its value.
        let last = &bytes[bytes.len() - 8..bytes.len() - 6];
        assert_eq!(half::f16::from_le_bytes([last[0], last[1]]).to_f32(), 1.0);
        assert_eq!(MIPS, 7);
    }
}
