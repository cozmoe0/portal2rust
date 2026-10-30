//! The paint on the world's surfaces. A map with `paintinmap 1` gets a
//! paint map: a byte per lightmap luxel holding the paint type (top three
//! bits) and amount (low five bits, out of 31), laid out on the lightmap
//! atlas so the world shader can draw the paint over the surfaces it
//! covers. The game's blobs paint spheres into it and the player's gel
//! powers read it back where the player touches the world. The rules follow
//! the game's (docs/re/paint.md).

pub mod upload;

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use source_formats::bsp::{Bsp, surf};

use crate::bsp_mesh::WorldGeometry;
use portal2_core::console::Console;
use portal2_core::vfs::GameFs;
use portal2_render::materials::{PaintTextures, load_vmt};
use portal2_render::textures::TextureCache;

pub use upload::PaintUploads;

pub struct PaintMapPlugin;

impl Plugin for PaintMapPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(upload::PaintUploadPlugin).add_systems(PostUpdate, queue_uploads);
    }
}

/// Paint types as the game numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PaintType {
    Bounce = 0,
    /// Never placed in the campaign; drawn with the portal colour.
    Reflect = 1,
    Speed = 2,
    Portal = 3,
    Erase = 4,
}

impl PaintType {
    pub fn from_index(i: i32) -> Option<Self> {
        match i {
            0 => Some(Self::Bounce),
            1 => Some(Self::Reflect),
            2 => Some(Self::Speed),
            3 => Some(Self::Portal),
            4 => Some(Self::Erase),
            _ => None,
        }
    }

    fn color_cvar(self) -> &'static str {
        match self {
            Self::Bounce => "bounce_paint_color",
            Self::Speed => "speed_paint_color",
            Self::Reflect | Self::Portal => "portal_paint_color",
            Self::Erase => "erase_color",
        }
    }

    /// The colour the paint map stores for this type (sRGB with alpha).
    pub fn color(self, console: &Console) -> [u8; 4] {
        let mut out = [0u8, 0, 0, 255];
        for (i, v) in console.str(self.color_cvar()).split_whitespace().take(4).enumerate() {
            out[i] = v.parse::<f32>().unwrap_or(0.0).clamp(0.0, 255.0) as u8;
        }
        out
    }
}

const TYPE_SHIFT: u32 = 5;
const AMOUNT_MASK: u8 = 0x1f;
const AMOUNT_STEPS: f32 = 31.0;

fn luxel_type(b: u8) -> u8 {
    b >> TYPE_SHIFT
}

fn luxel_amount(b: u8) -> f32 {
    (b & AMOUNT_MASK) as f32 / AMOUNT_STEPS
}

/// A face that can take paint.
pub struct PaintFace {
    /// BSP face index.
    pub face: usize,
    pub normal: Vec3,
    pub dist: f32,
    pub verts: Vec<Vec3>,
    pub mins: Vec3,
    pub maxs: Vec3,
    lm_s: Vec4,
    lm_t: Vec4,
    lm_mins: Vec2,
    /// Highest luxel index on each axis (luxels run 0..=size).
    pub size: UVec2,
    /// Atlas texel holding luxel (0, 0).
    pub origin: UVec2,
    /// Type and amount per luxel, row-major.
    pub luxels: Vec<u8>,
    dirty: bool,
}

impl PaintFace {
    fn row(&self) -> u32 {
        self.size.x + 1
    }

    fn index(&self, x: u32, y: u32) -> usize {
        (y * self.row() + x) as usize
    }

    /// Lightmap position of `p` relative to the face's lightmap mins, in luxels.
    fn luxel_pos(&self, p: Vec3) -> Vec2 {
        Vec2::new(p.dot(self.lm_s.truncate()) + self.lm_s.w, p.dot(self.lm_t.truncate()) + self.lm_t.w) - self.lm_mins
    }
}

/// Paintable faces of one brush model, in the model's space.
pub struct ModelFaces {
    pub faces: Vec<usize>,
    pub mins: Vec3,
    pub maxs: Vec3,
}

/// The current map's paint.
#[derive(Resource)]
pub struct PaintMap {
    pub faces: Vec<PaintFace>,
    /// By brush model index (0 is the world).
    pub models: Vec<ModelFaces>,
    pub image: Handle<Image>,
    /// Most blobs alive at once (worldspawn `maxblobcount`).
    pub max_blobs: usize,
    rng: u32,
}

/// Splat settings read from the console once per batch of paint.
#[derive(Clone, Copy, Debug)]
pub struct SplatSettings {
    /// Exponent of the edge falloff (`paintsplat_bias` as a Bias curve).
    exponent: f32,
    noise: f32,
    border_max: f32,
    spread_type: bool,
}

impl SplatSettings {
    pub fn from_console(console: &Console) -> Self {
        let bias = console.f32("paintsplat_bias").clamp(1e-4, 0.9999);
        Self {
            exponent: bias.ln() / 0.5f32.ln(),
            noise: if console.bool("paintsplat_noise_enabled") { console.f32("paintsplat_max_alpha_noise") } else { 0.0 },
            border_max: console.f32("paint_max_surface_border_alpha"),
            spread_type: console.bool("paint_alpha_offset_enabled"),
        }
    }
}

impl Default for SplatSettings {
    fn default() -> Self {
        Self { exponent: 0.1f32.ln() / 0.5f32.ln(), noise: 0.1, border_max: 0.7, spread_type: true }
    }
}

/// One sphere of paint: its circle on one face, in luxels.
struct Circle {
    cx: f32,
    cy: f32,
    radius: f32,
    offset: f32,
    /// Luxel rectangle the circle may touch (inclusive).
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

/// Uploads of changed faces: atlas rectangle plus RGBA texels.
pub struct PaintRect {
    pub image: AssetId<Image>,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// Size of one texel in `data` (4 for the paint map's RGBA8, 8 for
    /// the lightmap atlas's RGBA16F).
    pub texel_bytes: u32,
    pub data: Vec<u8>,
}

impl PaintMap {
    fn random(&mut self) -> f32 {
        // xorshift32
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Faces of `model` a sphere of paint at `center` (model space) covers:
    /// those in front of it within `radius` whose polygon the sphere
    /// touches, minus those behind the plane of the closest one.
    fn sphere_faces(&self, model: usize, center: Vec3, radius: f32) -> Vec<usize> {
        let Some(m) = self.models.get(model) else { return Vec::new() };
        if !sphere_touches_box(center, radius, m.mins, m.maxs) {
            return Vec::new();
        }
        let mut found = Vec::new();
        let mut closest: Option<(f32, Vec3, f32)> = None;
        for &fi in &m.faces {
            let f = &self.faces[fi];
            if !sphere_touches_box(center, radius, f.mins, f.maxs) {
                continue;
            }
            let d = f.normal.dot(center) - f.dist;
            if !(0.0..=radius).contains(&d) {
                continue;
            }
            let mut best: Option<f32> = None;
            for i in 1..f.verts.len() - 1 {
                let mut q = closest_on_triangle(center, f.verts[0], f.verts[i], f.verts[i + 1]);
                let dq = f.normal.dot(q) - f.dist;
                if dq < 0.0 {
                    q += f.normal * (dq + 0.1);
                }
                let d2 = q.distance_squared(center);
                if d2 < radius * radius && best.is_none_or(|b| d2 < b) {
                    best = Some(d2);
                }
            }
            let Some(d2) = best else { continue };
            if closest.is_none_or(|c| d2 < c.0) {
                closest = Some((d2, f.normal, f.dist));
            }
            found.push(fi);
        }
        let Some((_, cn, cd)) = closest else { return Vec::new() };
        found.retain(|&fi| {
            let f = &self.faces[fi];
            if (f.normal.dot(cn) - 1.0).abs() < 1e-5 && (f.dist - cd).abs() < 1e-3 {
                return true;
            }
            let p = f.verts[0] + ((f.verts[1] - f.verts[0]) + (f.verts[2] - f.verts[0])) * 0.25 + f.normal * 0.1;
            cn.dot(p) - cd >= 0.0
        });
        found
    }

    fn circle(f: &PaintFace, center: Vec3, radius: f32) -> Option<Circle> {
        let d = f.normal.dot(center) - f.dist;
        let on_plane = (radius * radius - d * d).max(0.0).sqrt();
        let (ls, lt) = (f.lm_s.truncate().length(), f.lm_t.truncate().length());
        let c = f.luxel_pos(center) + Vec2::splat(0.5);
        let (w, h) = (f.size.x as f32, f.size.y as f32);
        if c.x + ls * on_plane < 0.0 || w < c.x - ls * on_plane || c.y + lt * on_plane < 0.0 || h < c.y - lt * on_plane {
            return None;
        }
        let r = ((ls * on_plane).max(lt * on_plane) + 0.5).floor();
        let x0 = ((c.x - r - 0.5).floor() as i32).max(0);
        let y0 = ((c.y - r - 0.5).floor() as i32).max(0);
        let x1 = ((c.x + r + 0.5).floor() as i32).min(f.size.x as i32 + 1) - 1;
        let y1 = ((c.y + r + 0.5).floor() as i32).min(f.size.y as i32 + 1) - 1;
        if x1 < x0 || y1 < y0 {
            return None;
        }
        Some(Circle { cx: c.x, cy: c.y, radius: r, offset: d / radius, x0, y0, x1, y1 })
    }

    /// Paints a sphere into `model` (model-space `center`). Returns whether
    /// any luxel changed.
    pub fn paint_sphere(&mut self, model: usize, center: Vec3, radius: f32, coat: f32, ty: PaintType, s: &SplatSettings) -> bool {
        let mut changed = false;
        for fi in self.sphere_faces(model, center, radius) {
            let Some(circle) = Self::circle(&self.faces[fi], center, radius) else { continue };
            changed |= self.paint_circle(fi, &circle, coat, ty, s);
        }
        changed
    }

    /// Rasterises the circle as the spans of a midpoint circle.
    fn paint_circle(&mut self, fi: usize, c: &Circle, coat: f32, ty: PaintType, s: &SplatSettings) -> bool {
        let mut changed = false;
        let (mut x, mut y, mut err) = (c.radius, 0.0f32, -c.radius);
        if x < 0.0 {
            return false;
        }
        loop {
            changed |= self.paint_span(fi, c, x, y, coat, ty, s);
            if x != y {
                changed |= self.paint_span(fi, c, y, x, coat, ty, s);
            }
            err += y;
            y += 1.0;
            err += y;
            if err >= 0.0 {
                x -= 1.0;
                err -= x + x;
            }
            if x < y {
                break;
            }
        }
        changed
    }

    /// Rows `cy ± dy` from `cx - dx` to `cx + dx`, ends pushed out half a luxel.
    fn paint_span(&mut self, fi: usize, c: &Circle, dx: f32, dy: f32, coat: f32, ty: PaintType, s: &SplatSettings) -> bool {
        let xa = ((c.cx - dx - 0.5).floor() as i32).max(c.x0);
        let xb = ((c.cx + dx + 0.5).floor() as i32).min(c.x1);
        let ya = ((c.cy - dy - 0.5).floor() as i32).max(c.y0);
        let yb = ((c.cy + dy + 0.5).floor() as i32).min(c.y1);
        if xa > c.x1 || xb < c.x0 {
            return false;
        }
        let mut changed = false;
        if (c.y0..=c.y1).contains(&ya) {
            changed |= self.paint_row(fi, c, xa, xb, ya, coat, ty, s);
        }
        if ya != yb && (c.y0..=c.y1).contains(&yb) {
            changed |= self.paint_row(fi, c, xa, xb, yb, coat, ty, s);
        }
        changed
    }

    fn paint_row(&mut self, fi: usize, c: &Circle, xa: i32, xb: i32, y: i32, coat: f32, ty: PaintType, s: &SplatSettings) -> bool {
        let (sx, sy) = (self.faces[fi].size.x as i32, self.faces[fi].size.y as i32);
        let mut changed = false;
        let mut paint = |map: &mut Self, x: i32, max: f32| {
            let amount = map.splat_amount(c, x, y, ty, s);
            changed |= map.add_paint(fi, x as u32, y as u32, amount * coat, max, ty);
        };
        if y == 0 || y == sy {
            for x in xa..=xb {
                paint(self, x, s.border_max);
            }
        } else {
            let (mut from, mut to) = (xa, xb);
            if xa == 0 {
                paint(self, xa, s.border_max);
                from = xa + 1;
            }
            if xb == sx {
                paint(self, xb, s.border_max);
                to = xb - 1;
            }
            for x in from..=to {
                paint(self, x, 1.0);
            }
        }
        if s.spread_type && ty != PaintType::Erase {
            changed |= self.spread_type(fi, xa, y, ty);
            if xa != xb {
                changed |= self.spread_type(fi, xb, y, ty);
            }
        }
        changed
    }

    /// Amount a luxel of the circle receives (before the coat): near 1 in
    /// the middle, falling steeply at the edge, with noise.
    fn splat_amount(&mut self, c: &Circle, x: i32, y: i32, ty: PaintType, s: &SplatSettings) -> f32 {
        if ty == PaintType::Erase {
            return 1.0;
        }
        let dist = Vec2::new(x as f32 - c.cx, y as f32 - c.cy).length();
        let t = if c.radius > 0.0 { dist / c.radius + c.offset } else { c.offset }.clamp(0.0, 1.0);
        let mut a = 1.0 - t.powf(s.exponent);
        if s.noise > 0.0 {
            a = (a + (self.random() * 2.0 - 1.0) * s.noise).clamp(0.0, 1.0);
        }
        a
    }

    fn add_paint(&mut self, fi: usize, x: u32, y: u32, add: f32, max: f32, ty: PaintType) -> bool {
        let f = &mut self.faces[fi];
        let i = f.index(x, y);
        let old = f.luxels[i];
        let new = if ty == PaintType::Erase {
            old & !AMOUNT_MASK
        } else {
            let amount = (luxel_amount(old) + add).clamp(0.0, max);
            ((ty as u8) << TYPE_SHIFT) | (amount * AMOUNT_STEPS) as u8
        };
        if new == old {
            return false;
        }
        f.luxels[i] = new;
        f.dirty = true;
        true
    }

    /// The eight neighbours of a span end take the new type (keeping their
    /// amount), so the colour filters cleanly across the splat's edge.
    fn spread_type(&mut self, fi: usize, x: i32, y: i32, ty: PaintType) -> bool {
        let f = &mut self.faces[fi];
        let (sx, sy) = (f.size.x as i32, f.size.y as i32);
        let mut changed = false;
        for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx > sx || ny > sy {
                continue;
            }
            let i = f.index(nx as u32, ny as u32);
            let new = ((ty as u8) << TYPE_SHIFT) | (f.luxels[i] & AMOUNT_MASK);
            if new != f.luxels[i] {
                f.luxels[i] = new;
                f.dirty = true;
                changed = true;
            }
        }
        changed
    }

    /// Paint types under a sphere (model space): counts of luxels per type
    /// holding more than `min_amount`, over the circles the sphere draws on
    /// the faces it would paint.
    pub fn query(&self, model: usize, center: Vec3, radius: f32, min_amount: f32) -> [u32; 5] {
        let mut counts = [0u32; 5];
        for fi in self.sphere_faces(model, center, radius) {
            let f = &self.faces[fi];
            let Some(c) = Self::circle(f, center, radius) else { continue };
            for y in c.y0..=c.y1 {
                for x in c.x0..=c.x1 {
                    let b = f.luxels[f.index(x as u32, y as u32)];
                    if luxel_amount(b) > min_amount
                        && let Some(n) = counts.get_mut(luxel_type(b) as usize)
                    {
                        *n += 1;
                    }
                }
            }
        }
        counts
    }

    /// The paint type most of the luxels under a sphere carry (see `query`).
    pub fn dominant(&self, model: usize, center: Vec3, radius: f32, min_amount: f32) -> Option<PaintType> {
        let counts = self.query(model, center, radius, min_amount);
        let (i, &n) = counts.iter().enumerate().take(4).max_by_key(|(_, n)| **n)?;
        if n == 0 { None } else { PaintType::from_index(i as i32) }
    }

    /// RGBA blocks of the faces changed since the last call, with the
    /// one-texel border repeating the edge (like the lightmaps).
    fn take_uploads(&mut self, colors: &[[u8; 4]; 5]) -> Vec<PaintRect> {
        let image = self.image.id();
        let mut out = Vec::new();
        for f in self.faces.iter_mut().filter(|f| f.dirty) {
            f.dirty = false;
            trace!("paint upload: face {} near {}", f.face, (f.mins + f.maxs) * 0.5);
            let (w, h) = (f.size.x + 3, f.size.y + 3);
            let mut data = Vec::with_capacity((w * h * 4) as usize);
            for ty in 0..h {
                let sy = (ty as i32 - 1).clamp(0, f.size.y as i32) as u32;
                for tx in 0..w {
                    let sx = (tx as i32 - 1).clamp(0, f.size.x as i32) as u32;
                    let b = f.luxels[(sy * (f.size.x + 1) + sx) as usize];
                    let c = colors.get(luxel_type(b) as usize).copied().unwrap_or([0; 4]);
                    data.extend_from_slice(&[c[0], c[1], c[2], (luxel_amount(b) * 255.0) as u8]);
                }
            }
            out.push(PaintRect { image, x: f.origin.x - 1, y: f.origin.y - 1, width: w, height: h, texel_bytes: 4, data });
        }
        out
    }

    /// Every face that holds paint, for saves.
    pub fn snapshot(&self) -> Vec<(usize, Vec<u8>)> {
        self.faces.iter().filter(|f| f.luxels.iter().any(|b| b & AMOUNT_MASK != 0)).map(|f| (f.face, f.luxels.clone())).collect()
    }

    /// Replaces all paint with a snapshot.
    pub fn restore(&mut self, saved: &[(usize, Vec<u8>)]) {
        let by_face: HashMap<usize, &Vec<u8>> = saved.iter().map(|(f, l)| (*f, l)).collect();
        for f in &mut self.faces {
            match by_face.get(&f.face) {
                Some(l) if l.len() == f.luxels.len() => f.luxels.copy_from_slice(l),
                _ => f.luxels.fill(0),
            }
            f.dirty = true;
        }
    }
}

fn sphere_touches_box(c: Vec3, r: f32, mins: Vec3, maxs: Vec3) -> bool {
    c.clamp(mins, maxs).distance_squared(c) <= r * r
}

/// Closest point to `p` on triangle `abc`.
fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

/// Whether a material is drawn translucent (such faces never take paint).
fn material_is_translucent(fs: &GameFs, name: &str) -> bool {
    let Some(vmt) = load_vmt(fs, &source_formats::vmt::material_path(name)) else { return false };
    vmt.get_bool("$translucent") || vmt.get_bool("$additive")
}

/// The paint map of a map with `paintinmap 1` (all faces bare), with its
/// GPU texture the size of the lightmap atlas.
pub fn build_paint_map(bsp: &Bsp, geometry: &WorldGeometry, fs: &GameFs, images: &mut Assets<Image>) -> Option<PaintMap> {
    let world = bsp.entities.first()?;
    if world.get_i32("paintinmap").unwrap_or(0) == 0 {
        return None;
    }
    let mut translucent: HashMap<i32, bool> = HashMap::new();
    let mut faces = Vec::new();
    let mut models = Vec::with_capacity(bsp.models.len());
    for model in &bsp.models {
        let mut list = Vec::new();
        let (mut mins, mut maxs) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for fi in model.first_face..model.first_face + model.num_faces {
            let fi = fi as usize;
            let face = &bsp.faces[fi];
            let Some(&origin) = geometry.face_luxels.get(&fi) else { continue };
            if face.dispinfo >= 0 || face.texinfo < 0 {
                continue;
            }
            let ti = &bsp.texinfo[face.texinfo as usize];
            // The no-decals flag also keeps paint off (the engine marks
            // such faces unpaintable).
            if ti.flags & surf::NODECALS != 0 {
                continue;
            }
            let td = &bsp.texdata[ti.texdata as usize];
            let see_through = *translucent
                .entry(ti.texdata)
                .or_insert_with(|| bsp.texdata_strings.get(td.name_index as usize).is_some_and(|n| material_is_translucent(fs, n)));
            if see_through {
                continue;
            }
            let verts = bsp.face_vertices(face);
            if verts.len() < 3 {
                continue;
            }
            let plane = &bsp.planes[face.plane as usize];
            let (fmin, fmax) = verts.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), v| (a.min(*v), b.max(*v)));
            mins = mins.min(fmin);
            maxs = maxs.max(fmax);
            let size = UVec2::new(face.lightmap_size[0].max(0) as u32, face.lightmap_size[1].max(0) as u32);
            list.push(faces.len());
            faces.push(PaintFace {
                face: fi,
                normal: plane.normal,
                dist: plane.dist,
                verts,
                mins: fmin,
                maxs: fmax,
                lm_s: Vec4::from_array(ti.lightmap_vecs[0]),
                lm_t: Vec4::from_array(ti.lightmap_vecs[1]),
                lm_mins: Vec2::new(face.lightmap_mins[0] as f32, face.lightmap_mins[1] as f32),
                size,
                origin: UVec2::from_array(origin),
                luxels: vec![0; ((size.x + 1) * (size.y + 1)) as usize],
                dirty: false,
            });
        }
        models.push(ModelFaces { faces: list, mins, maxs });
    }
    let atlas = UVec2::new(geometry.atlas.width, geometry.atlas.height);
    let mut image = Image::new_fill(
        Extent3d { width: atlas.x, height: atlas.y, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0, 0, 0, 0],
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    let max_blobs = world.get_i32("maxblobcount").unwrap_or(250).max(0) as usize;
    info!("paint map: {} paintable faces, up to {max_blobs} blobs", faces.len());
    Some(PaintMap { faces, models, image: images.add(image), max_blobs, rng: 0x2545_f491 })
}

/// Textures the paint pass reads, for the map's world materials.
pub fn paint_textures(map: &PaintMap, fs: &GameFs, textures: &mut TextureCache, images: &mut Assets<Image>) -> PaintTextures {
    let mut tex = |name: &str| textures.load(fs, images, &format!("materials/paint/{name}.vtf"), false).map(|t| t.handle);
    PaintTextures {
        map: map.image.clone(),
        splat: tex("splatnormal_default"),
        bubble_layout: tex("bubblelayout"),
        bubble: tex("bubble"),
        envmap: tex("paint_envmap_hdr"),
    }
}

/// Sends changed faces to the GPU.
fn queue_uploads(map: Option<ResMut<PaintMap>>, console: Res<Console>, mut uploads: ResMut<PaintUploads>) {
    let Some(mut map) = map else { return };
    if !map.faces.iter().any(|f| f.dirty) {
        return;
    }
    let colors = [PaintType::Bounce, PaintType::Reflect, PaintType::Speed, PaintType::Portal, PaintType::Erase].map(|t| t.color(&console));
    let rects = map.take_uploads(&colors);
    uploads.push(rects);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 16-luxel floor face 256 units wide at z = 0 (16 units per luxel).
    fn floor() -> PaintMap {
        let verts = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 256.0, 0.0), Vec3::new(256.0, 256.0, 0.0), Vec3::new(256.0, 0.0, 0.0)];
        let size = UVec2::new(16, 16);
        let face = PaintFace {
            face: 0,
            normal: Vec3::Z,
            dist: 0.0,
            verts,
            mins: Vec3::new(0.0, 0.0, 0.0),
            maxs: Vec3::new(256.0, 256.0, 0.0),
            lm_s: Vec4::new(1.0 / 16.0, 0.0, 0.0, 0.0),
            lm_t: Vec4::new(0.0, 1.0 / 16.0, 0.0, 0.0),
            lm_mins: Vec2::ZERO,
            size,
            origin: UVec2::new(1, 1),
            luxels: vec![0; 17 * 17],
            dirty: false,
        };
        PaintMap {
            faces: vec![face],
            models: vec![ModelFaces { faces: vec![0], mins: Vec3::ZERO, maxs: Vec3::new(256.0, 256.0, 0.0) }],
            image: Handle::default(),
            max_blobs: 250,
            rng: 1,
        }
    }

    fn quiet() -> SplatSettings {
        SplatSettings { noise: 0.0, ..default() }
    }

    #[test]
    fn splat_is_full_in_the_middle_and_fades_out() {
        let mut map = floor();
        assert!(map.paint_sphere(0, Vec3::new(128.0, 128.0, 0.1), 60.0, 0.4, PaintType::Bounce, &quiet()));
        let f = &map.faces[0];
        // The centre luxel gets nearly the whole coat; 0.4 × 31 = 12 steps.
        let centre = f.luxels[f.index(8, 8)];
        assert_eq!(luxel_type(centre), 0);
        assert_eq!(centre & AMOUNT_MASK, 12);
        // Radius 60 is under 4 luxels: 5 luxels away stays bare.
        assert_eq!(f.luxels[f.index(13, 8)] & AMOUNT_MASK, 0);
        // Coats add up to full and stop there.
        for _ in 0..5 {
            map.paint_sphere(0, Vec3::new(128.0, 128.0, 0.1), 60.0, 0.4, PaintType::Speed, &quiet());
        }
        let f = &map.faces[0];
        assert_eq!(f.luxels[f.index(8, 8)], (2 << TYPE_SHIFT) | 31);
    }

    #[test]
    fn border_luxels_stop_at_the_border_amount() {
        let mut map = floor();
        for _ in 0..6 {
            map.paint_sphere(0, Vec3::new(8.0, 128.0, 0.1), 60.0, 0.4, PaintType::Bounce, &quiet());
        }
        let f = &map.faces[0];
        assert_eq!(f.luxels[f.index(0, 8)] & AMOUNT_MASK, (0.7 * 31.0) as u8);
        assert_eq!(f.luxels[f.index(1, 8)] & AMOUNT_MASK, 31);
    }

    #[test]
    fn back_side_is_not_painted_and_query_reads_paint() {
        let mut map = floor();
        assert!(!map.paint_sphere(0, Vec3::new(128.0, 128.0, -10.0), 60.0, 0.4, PaintType::Bounce, &quiet()));
        for _ in 0..3 {
            map.paint_sphere(0, Vec3::new(128.0, 128.0, 0.1), 60.0, 0.4, PaintType::Speed, &quiet());
        }
        assert_eq!(map.dominant(0, Vec3::new(128.0, 128.0, 1.0), 16.0, 0.7), Some(PaintType::Speed));
        assert_eq!(map.dominant(0, Vec3::new(20.0, 20.0, 1.0), 16.0, 0.7), None);
        // Erasing keeps the type but clears the amount.
        map.paint_sphere(0, Vec3::new(128.0, 128.0, 0.1), 80.0, 0.4, PaintType::Erase, &quiet());
        let f = &map.faces[0];
        assert_eq!(f.luxels[f.index(8, 8)], 2 << TYPE_SHIFT);
    }

    #[test]
    fn closest_point_on_triangle_regions() {
        let (a, b, c) = (Vec3::ZERO, Vec3::X * 10.0, Vec3::Y * 10.0);
        let near = |p: Vec3, q: Vec3| p.distance(q) < 1e-4;
        assert!(near(closest_on_triangle(Vec3::new(2.0, 2.0, 5.0), a, b, c), Vec3::new(2.0, 2.0, 0.0)));
        assert!(near(closest_on_triangle(Vec3::new(-3.0, -3.0, 0.0), a, b, c), a));
        assert!(near(closest_on_triangle(Vec3::new(5.0, -4.0, 0.0), a, b, c), Vec3::new(5.0, 0.0, 0.0)));
    }
}
