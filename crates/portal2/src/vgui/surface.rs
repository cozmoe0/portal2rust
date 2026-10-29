//! The 2D painter all interface code draws with, in the spirit of the
//! game's `ISurface`: filled and textured rectangles, text in scheme fonts,
//! clip rectangles and a few draw layers. Geometry is collected per frame
//! in screen pixels and handed to `render` once painting is done.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::asset::uuid::Uuid;
use bevy::image::{ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use source_formats::fs::GameFileSystem;

use super::font::{Abc, FontSpec, RasterFont};
use super::render::{Batch, Blend, VguiFrame, VguiVertex};

/// Draw layers, painted in this order.
pub mod layer {
    /// Screen fades and other view overlays.
    pub const VIEW: usize = 0;
    pub const HUD: usize = 1;
    /// Menus, dialogs and the loading screen.
    pub const GAMEUI: usize = 2;
    pub const CONSOLE: usize = 3;
    pub const TOP: usize = 4;
    pub const COUNT: usize = 5;
}

/// RGBA, 0-255, as the game's `Color`.
pub type Rgba = [u8; 4];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontId(pub u32);

/// A texture the painter can draw: a loaded VGUI material or a frame
/// someone else fills (videos).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageRef {
    pub id: AssetId<Image>,
    pub width: u32,
    pub height: u32,
    pub additive: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tex {
    White,
    Page(u32),
    Image(AssetId<Image>),
}

#[derive(Clone, Copy, Debug)]
struct RawBatch {
    tex: Tex,
    blend: Blend,
    start: u32,
    count: u32,
}

#[derive(Default)]
struct LayerGeometry {
    verts: Vec<VguiVertex>,
    batches: Vec<RawBatch>,
}

const PAGE_SIZE: u32 = 512;

/// A glyph atlas page; glyphs are packed into rows of equal height.
struct Page {
    id: AssetId<Image>,
    data: Vec<u8>,
    rows: Vec<(u32, u32, u32)>, // y, height, next free x
    next_y: u32,
    dirty: bool,
}

#[derive(Clone, Copy)]
struct GlyphSlot {
    page: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

struct LoadedFont {
    raster: RasterFont,
    glyphs: HashMap<char, Option<GlyphSlot>>,
}

#[derive(Resource)]
pub struct Surface {
    pub wide: i32,
    pub tall: i32,
    fs: Option<Arc<GameFileSystem>>,
    layer: usize,
    layers: Vec<LayerGeometry>,
    clip: Vec<[i32; 4]>,
    /// Multiplies the alpha of everything drawn (panel fades).
    pub alpha: f32,
    /// This frame shows the paused-game backdrop under the interface.
    pub backdrop: bool,
    fonts: Vec<LoadedFont>,
    by_spec: HashMap<FontSpec, FontId>,
    pages: Vec<Page>,
    images: HashMap<String, Option<ImageRef>>,
    pending: Vec<(AssetId<Image>, Image)>,
    next_uuid: u128,
    white: AssetId<Image>,
    white_uploaded: bool,
}

impl Default for Surface {
    fn default() -> Self {
        let mut s = Self {
            wide: 640,
            tall: 480,
            fs: None,
            layer: layer::HUD,
            layers: (0..layer::COUNT).map(|_| LayerGeometry::default()).collect(),
            clip: Vec::new(),
            alpha: 1.0,
            backdrop: false,
            fonts: Vec::new(),
            by_spec: HashMap::new(),
            pages: Vec::new(),
            images: HashMap::new(),
            pending: Vec::new(),
            next_uuid: 0x5652_4755_4900_0000_0000_0000_0000_0000,
            white: AssetId::default(),
            white_uploaded: false,
        };
        s.white = s.new_id();
        s
    }
}

impl Surface {
    pub fn set_filesystem(&mut self, fs: Arc<GameFileSystem>) {
        self.fs = Some(fs);
    }

    pub fn filesystem(&self) -> Option<&Arc<GameFileSystem>> {
        self.fs.as_ref()
    }

    fn new_id(&mut self) -> AssetId<Image> {
        self.next_uuid += 1;
        AssetId::Uuid { uuid: Uuid::from_u128(self.next_uuid) }
    }

    /// An id for an image someone else will insert into `Assets<Image>`.
    pub fn reserve_image_id(&mut self) -> AssetId<Image> {
        self.new_id()
    }

    pub fn set_layer(&mut self, layer: usize) {
        self.layer = layer.min(layer::COUNT - 1);
    }

    pub fn layer(&self) -> usize {
        self.layer
    }

    // ------------------------------------------------------------ clipping

    /// Restricts drawing to `x0..x1, y0..y1` (intersected with any outer clip).
    pub fn push_clip(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        let mut r = [x0, y0, x1, y1];
        if let Some(outer) = self.clip.last() {
            r = [r[0].max(outer[0]), r[1].max(outer[1]), r[2].min(outer[2]), r[3].min(outer[3])];
        }
        self.clip.push(r);
    }

    pub fn pop_clip(&mut self) {
        self.clip.pop();
    }

    // ------------------------------------------------------------- shapes

    pub fn fill(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Rgba) {
        self.quad(Tex::White, Blend::Alpha, [x0 as f32, y0 as f32, x1 as f32, y1 as f32], [0.0, 0.0, 1.0, 1.0], [color; 4]);
    }

    /// Multiplies what is under the rectangle by `color` (alpha ignored).
    pub fn modulate(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Rgba) {
        let c = [color[0], color[1], color[2], 255];
        self.quad(Tex::White, Blend::Modulate, [x0 as f32, y0 as f32, x1 as f32, y1 as f32], [0.0, 0.0, 1.0, 1.0], [c; 4]);
    }

    /// A rectangle whose alpha goes from `alpha0` to `alpha1` across it
    /// (`horizontal`) or down it, like `DrawFilledRectFade`.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_fade(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Rgba, alpha0: u8, alpha1: u8, horizontal: bool) {
        let a = |alpha: u8| [color[0], color[1], color[2], ((color[3] as u32 * alpha as u32) / 255) as u8];
        let (c0, c1) = (a(alpha0), a(alpha1));
        let colors = if horizontal { [c0, c1, c1, c0] } else { [c0, c0, c1, c1] };
        self.quad(Tex::White, Blend::Alpha, [x0 as f32, y0 as f32, x1 as f32, y1 as f32], [0.0, 0.0, 1.0, 1.0], colors);
    }

    /// A one pixel frame just inside the rectangle.
    pub fn outline(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Rgba) {
        self.fill(x0, y0, x1, y0 + 1, color);
        self.fill(x0, y1 - 1, x1, y1, color);
        self.fill(x0, y0 + 1, x0 + 1, y1 - 1, color);
        self.fill(x1 - 1, y0 + 1, x1, y1 - 1, color);
    }

    // ------------------------------------------------------------- images

    /// A VGUI material by the name panels use (`vgui/...` or a path under
    /// `materials/`), loaded on first use.
    pub fn image(&mut self, name: &str) -> Option<ImageRef> {
        let key = name.to_ascii_lowercase().replace('\\', "/");
        if let Some(cached) = self.images.get(&key) {
            return *cached;
        }
        let loaded = self.load_image(&key);
        self.images.insert(key, loaded);
        loaded
    }

    fn load_image(&mut self, name: &str) -> Option<ImageRef> {
        let fs = self.fs.clone()?;
        let (image, width, height, additive) = super::image::load(&fs, name)?;
        let id = self.new_id();
        self.pending.push((id, image));
        Some(ImageRef { id, width, height, additive })
    }

    /// Draws `image` stretched over the rectangle.
    pub fn draw_image(&mut self, image: &ImageRef, x0: i32, y0: i32, x1: i32, y1: i32, color: Rgba) {
        self.draw_image_uv(image, x0 as f32, y0 as f32, x1 as f32, y1 as f32, [0.0, 0.0, 1.0, 1.0], color);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_image_uv(&mut self, image: &ImageRef, x0: f32, y0: f32, x1: f32, y1: f32, uv: [f32; 4], color: Rgba) {
        let blend = if image.additive { Blend::Additive } else { Blend::Alpha };
        self.quad(Tex::Image(image.id), blend, [x0, y0, x1, y1], uv, [color; 4]);
    }

    // -------------------------------------------------------------- fonts

    pub fn font(&mut self, spec: FontSpec) -> FontId {
        if let Some(id) = self.by_spec.get(&spec) {
            return *id;
        }
        let id = FontId(self.fonts.len() as u32);
        self.by_spec.insert(spec.clone(), id);
        self.fonts.push(LoadedFont { raster: RasterFont::new(spec), glyphs: HashMap::new() });
        id
    }

    pub fn font_spec(&self, font: FontId) -> Option<&FontSpec> {
        self.fonts.get(font.0 as usize).map(|f| f.raster.spec())
    }

    /// Cell height of the font (`GetFontTall`).
    pub fn font_tall(&self, font: FontId) -> i32 {
        self.fonts.get(font.0 as usize).map_or(0, |f| f.raster.metrics().height)
    }

    pub fn font_ascent(&self, font: FontId) -> i32 {
        self.fonts.get(font.0 as usize).map_or(0, |f| f.raster.metrics().ascent)
    }

    pub fn char_abc(&mut self, font: FontId, ch: char) -> Abc {
        self.fonts.get_mut(font.0 as usize).map_or(Abc::default(), |f| f.raster.abc(ch))
    }

    /// Advance of one character (`GetCharacterWidth`).
    pub fn char_width(&mut self, font: FontId, ch: char) -> i32 {
        self.char_abc(font, ch).advance()
    }

    pub fn text_width(&mut self, font: FontId, text: &str) -> i32 {
        text.chars().map(|c| self.char_width(font, c)).sum()
    }

    /// Draws one line of text with its cell tops at `y`; returns the pen
    /// position after it.
    pub fn draw_text(&mut self, font: FontId, x: i32, y: i32, text: &str, color: Rgba) -> i32 {
        let mut pen = x;
        for ch in text.chars() {
            pen += self.draw_char(font, pen, y, ch, color);
        }
        pen
    }

    /// Draws one character at the pen position; returns its advance.
    pub fn draw_char(&mut self, font: FontId, x: i32, y: i32, ch: char, color: Rgba) -> i32 {
        let abc = self.char_abc(font, ch);
        if ch != ' '
            && abc.b > 0
            && let Some(slot) = self.glyph(font, ch)
        {
            let additive = self.fonts[font.0 as usize].raster.spec().additive();
            let p = PAGE_SIZE as f32;
            let uv = [slot.x as f32 / p, slot.y as f32 / p, (slot.x + slot.w) as f32 / p, (slot.y + slot.h) as f32 / p];
            let (x0, y0) = ((x + abc.a) as f32, y as f32);
            let rect = [x0, y0, x0 + slot.w as f32, y0 + slot.h as f32];
            let blend = if additive { Blend::Additive } else { Blend::Alpha };
            self.quad(Tex::Page(slot.page), blend, rect, uv, [color; 4]);
        }
        abc.advance()
    }

    fn glyph(&mut self, font: FontId, ch: char) -> Option<GlyphSlot> {
        let f = self.fonts.get_mut(font.0 as usize)?;
        if let Some(slot) = f.glyphs.get(&ch) {
            return *slot;
        }
        let (w, h, rgba) = f.raster.rasterize(ch);
        let slot = if w > 0 && h > 0 { self.pack(w as u32, h as u32, &rgba) } else { None };
        self.fonts[font.0 as usize].glyphs.insert(ch, slot);
        slot
    }

    fn pack(&mut self, w: u32, h: u32, rgba: &[u8]) -> Option<GlyphSlot> {
        if w + 1 > PAGE_SIZE || h + 1 > PAGE_SIZE {
            return None;
        }
        let mut place = None;
        'pages: for (pi, page) in self.pages.iter_mut().enumerate() {
            for row in page.rows.iter_mut() {
                if row.1 == h && row.2 + w + 1 <= PAGE_SIZE {
                    place = Some((pi, row.2, row.0));
                    row.2 += w + 1;
                    break 'pages;
                }
            }
            if page.next_y + h + 1 <= PAGE_SIZE {
                page.rows.push((page.next_y, h, w + 1));
                place = Some((pi, 0, page.next_y));
                page.next_y += h + 1;
                break;
            }
        }
        let (pi, x, y) = match place {
            Some(p) => p,
            None => {
                let id = self.new_id();
                self.pages.push(Page {
                    id,
                    data: vec![0; (PAGE_SIZE * PAGE_SIZE * 4) as usize],
                    rows: vec![(0, h, w + 1)],
                    next_y: h + 1,
                    dirty: true,
                });
                (self.pages.len() - 1, 0, 0)
            }
        };
        let page = &mut self.pages[pi];
        for row in 0..h {
            let src = (row * w * 4) as usize;
            let dst = (((y + row) * PAGE_SIZE + x) * 4) as usize;
            page.data[dst..dst + (w * 4) as usize].copy_from_slice(&rgba[src..src + (w * 4) as usize]);
        }
        page.dirty = true;
        Some(GlyphSlot { page: pi as u32, x, y, w, h })
    }

    // ------------------------------------------------------------ geometry

    #[allow(clippy::too_many_arguments)]
    fn quad(&mut self, tex: Tex, blend: Blend, rect: [f32; 4], uv: [f32; 4], colors: [Rgba; 4]) {
        let [mut x0, mut y0, mut x1, mut y1] = rect;
        let [mut u0, mut v0, mut u1, mut v1] = uv;
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        if let Some(c) = self.clip.last() {
            let (cx0, cy0, cx1, cy1) = (c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32);
            if x1 <= cx0 || x0 >= cx1 || y1 <= cy0 || y0 >= cy1 {
                return;
            }
            let du = (u1 - u0) / (x1 - x0);
            let dv = (v1 - v0) / (y1 - y0);
            if x0 < cx0 {
                u0 += (cx0 - x0) * du;
                x0 = cx0;
            }
            if x1 > cx1 {
                u1 -= (x1 - cx1) * du;
                x1 = cx1;
            }
            if y0 < cy0 {
                v0 += (cy0 - y0) * dv;
                y0 = cy0;
            }
            if y1 > cy1 {
                v1 -= (y1 - cy1) * dv;
                y1 = cy1;
            }
        }
        let alpha = self.alpha.clamp(0.0, 1.0);
        let colors = colors.map(|c| [c[0], c[1], c[2], (c[3] as f32 * alpha) as u8]);
        if colors.iter().all(|c| c[3] == 0) {
            return;
        }
        let geo = &mut self.layers[self.layer];
        let start = geo.verts.len() as u32;
        let corners = [(x0, y0, u0, v0), (x1, y0, u1, v0), (x1, y1, u1, v1), (x0, y1, u0, v1)];
        for i in [0usize, 1, 2, 0, 2, 3] {
            let (x, y, u, v) = corners[i];
            geo.verts.push(VguiVertex { pos: [x, y], uv: [u, v], color: colors[i] });
        }
        match geo.batches.last_mut() {
            Some(b) if b.tex == tex && b.blend == blend && b.start + b.count == start => b.count += 6,
            _ => geo.batches.push(RawBatch { tex, blend, start, count: 6 }),
        }
    }

    /// Ends the frame: positions to clip space, layers in order, textures
    /// resolved; pending images and changed glyph pages go to `images`.
    pub fn finish(&mut self, frame: &mut VguiFrame, images: &mut Assets<Image>) {
        if !self.white_uploaded {
            self.white_uploaded = true;
            let _ = images.insert(self.white, rgba_image(1, 1, vec![255; 4], true));
        }
        for (id, image) in self.pending.drain(..) {
            let _ = images.insert(id, image);
        }
        for page in self.pages.iter_mut().filter(|p| p.dirty) {
            page.dirty = false;
            let _ = images.insert(page.id, rgba_image(PAGE_SIZE, PAGE_SIZE, page.data.clone(), true));
        }
        frame.vertices.clear();
        frame.batches.clear();
        frame.backdrop = std::mem::take(&mut self.backdrop);
        let (sx, sy) = (2.0 / self.wide.max(1) as f32, 2.0 / self.tall.max(1) as f32);
        for geo in &mut self.layers {
            let base = frame.vertices.len() as u32;
            frame.vertices.extend(geo.verts.drain(..).map(|mut v| {
                v.pos = [v.pos[0] * sx - 1.0, 1.0 - v.pos[1] * sy];
                v
            }));
            for b in geo.batches.drain(..) {
                let image = match b.tex {
                    Tex::White => self.white,
                    Tex::Page(p) => self.pages[p as usize].id,
                    Tex::Image(id) => id,
                };
                frame.batches.push(Batch { image, blend: b.blend, start: base + b.start, count: b.count });
            }
        }
        self.clip.clear();
        self.alpha = 1.0;
    }
}

/// An uncompressed, non-sRGB RGBA image: the interface blends in gamma
/// space, so texels must reach the shader unconverted.
pub fn rgba_image(w: u32, h: u32, data: Vec<u8>, nearest: bool) -> Image {
    let mut image = Image::new(
        Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = if nearest { ImageSampler::nearest() } else { ImageSampler::Descriptor(ImageSamplerDescriptor::linear()) };
    image
}
