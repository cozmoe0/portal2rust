//! `vgui_screen` with the `sp_progress_sign` panel: the lightboard by each
//! chamber's entrance showing the chamber number ("03/19"), a progress bar
//! and the icons of what the chamber tests (`docs/re/world_visuals.md`,
//! "Progress signs").
//!
//! The screen is a `width` × `height` rectangle laid out like the movie
//! screens (lower left corner at the origin, along +Y and +Z, facing +X)
//! showing the panel's `pixelswide` × `pixelshigh` pixels
//! (`scripts/vgui_screens.txt`). What it shows comes from the map's entry
//! in `scripts/vscripts/transitions/sp_lightboard_icons.txt`; when the
//! screen becomes active it plays the entry's startup flicker from
//! `lightboard_startup_sequences.txt`. The panel is drawn here on the CPU,
//! as the game's VGUI draws it, whenever what it shows changes.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, Face, TextureDimension, TextureFormat};
use serde::{Deserialize, Serialize};
use source_formats::kv::{self, KeyValues};

use crate::entities::SpawnHandlers;
use crate::entities::io::{InputCall, InputHandlers};
use portal2_core::clock::GameClock;
use portal2_core::entity::MapEntity;
use portal2_core::vfs::GameFs;
use portal2_render::source_material::{WorldMaterial, world_flags};
use portal2_ui::vgui::Schemes;
use portal2_ui::vgui::font::RasterFont;
use portal2_world::CurrentMap;

const PANEL: &str = "sp_progress_sign";
const ICONS_FILE: &str = "scripts/vscripts/transitions/sp_lightboard_icons.txt";
const STARTUPS_FILE: &str = "scripts/vscripts/transitions/lightboard_startup_sequences.txt";
/// Icon names in the order of the cells of `vgui/screens/P2_lightboard_vgui`
/// (five by five), from the first cell.
const ICON_NAMES: [&str; 23] = [
    "cube_drop",
    "cube_button",
    "cube_bonk",
    "drink_water",
    "goop",
    "crushers",
    "laser_cube",
    "turret",
    "turret_burn",
    "portal_fling",
    "plate_fling",
    "bridges",
    "bridge_block",
    "grinders",
    "tbeams",
    "paint_bounce",
    "paint_speed",
    "handoff",
    "button_stand",
    "laser_power",
    "portal_fling_2",
    "tbeam_polarity",
    "danger_field",
];
const MAX_ICONS: usize = 10;
/// Grey the board is drawn in when lit (dimmed to a quarter or a half
/// while it flickers).
const BOARD_GREY: u8 = 140;
/// Alpha of icons for tests the chamber does not have.
const ICON_DIM_ALPHA: f32 = 140.0;

pub struct ProgressSignPlugin;

impl Plugin for ProgressSignPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, paint_signs);
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("vgui_screen", spawn_screen);
        world.resource_mut::<InputHandlers>().register("vgui_screen", screen_input);
    }
}

/// A map's lightboard: chamber number, chamber count, startup sequence,
/// dirt overlay and up to ten icons (sheet cell and whether it is lit).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Board {
    level: i32,
    total: i32,
    startup: Startup,
    dirt: Option<usize>,
    icons: Vec<Option<(usize, bool)>>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct Startup {
    flicker_rate: (f32, f32),
    flicker_quick: f32,
    bg_flicker_length: f32,
    level_number_delay: f32,
    icon_delay: f32,
    progress_delay: f32,
}

impl Default for Startup {
    fn default() -> Self {
        // `normal_flicker`; every single-player map names its own.
        Self {
            flicker_rate: (0.2, 0.65),
            flicker_quick: 0.02,
            bg_flicker_length: 0.75,
            level_number_delay: 0.8,
            icon_delay: 1.0,
            progress_delay: 1.65,
        }
    }
}

impl Startup {
    fn length(&self) -> f32 {
        self.bg_flicker_length.max(self.level_number_delay).max(self.icon_delay).max(self.progress_delay)
    }
}

#[derive(Component, Clone, Debug, Serialize, Deserialize)]
pub struct ProgressSign {
    active: bool,
    board: Board,
    size: (u32, u32),
    /// The startup sequence: when it began, the last flicker, the time to
    /// the next one and to the quick dimming after it.
    starting: Option<f64>,
    last_flicker: f64,
    flicker_rate: f32,
    flicker_quick: f32,
    board_grey: u8,
    dirt_alpha: u8,
    rng: u32,
    #[serde(skip)]
    quad: Option<Entity>,
    #[serde(skip)]
    image: Handle<Image>,
    /// What the image was last drawn with.
    #[serde(skip)]
    drawn: Option<(u8, u8, u8, u8)>,
}

impl portal2_core::save::Restore for ProgressSign {
    fn keep(&mut self, live: Self) {
        self.quad = live.quad;
        self.image = live.image;
    }
}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<ProgressSign>("progress_sign");
}

/// The panel's pixel size from `scripts/vgui_screens.txt`.
fn panel_size(fs: &GameFs, panel: &str) -> Option<(u32, u32)> {
    let doc = kv::parse_document(&fs.read_string("scripts/vgui_screens.txt")?);
    let screens = doc.roots.iter().find(|r| r.key.eq_ignore_ascii_case("vgui_screens"))?;
    let p = screens.children().iter().find(|c| c.key.eq_ignore_ascii_case(panel))?;
    Some((p.get_f32("pixelswide")? as u32, p.get_f32("pixelshigh")? as u32))
}

fn child<'a>(kv: &'a KeyValues, key: &str) -> Option<&'a KeyValues> {
    kv.children().iter().find(|c| c.key.eq_ignore_ascii_case(key))
}

fn load_board(fs: &GameFs, map: &str) -> Option<Board> {
    let doc = kv::parse_document(&fs.read_string(ICONS_FILE)?);
    let root = doc.roots.iter().find(|r| r.key.eq_ignore_ascii_case("sp_lightboard_icons"))?;
    let entry = child(child(root, "maps")?, map)?;
    let startup = entry.get_str("startup").and_then(|name| load_startup(fs, name)).unwrap_or_default();
    let mut icons = Vec::new();
    for icon in child(entry, "icons").map(|c| c.children()).unwrap_or_default() {
        if icons.len() == MAX_ICONS {
            warn!("progress sign: too many icons for {map}");
            break;
        }
        let cell = ICON_NAMES.iter().position(|n| n.eq_ignore_ascii_case(&icon.key));
        let lit = icon.as_str().is_some_and(|v| kv::parse_i32_lenient(v) != 0);
        icons.push(cell.map(|c| (c, lit)));
    }
    Some(Board {
        level: entry.get_f32("level_number").unwrap_or(0.0) as i32,
        total: entry.get_f32("total_levels").unwrap_or(0.0) as i32,
        startup,
        dirt: entry.get_f32("dirt").map(|d| (d as i32).clamp(0, 2) as usize),
        icons,
    })
}

fn load_startup(fs: &GameFs, name: &str) -> Option<Startup> {
    let doc = kv::parse_document(&fs.read_string(STARTUPS_FILE)?);
    let root = doc.roots.iter().find(|r| r.key.eq_ignore_ascii_case("lightboard_startup_sequences"))?;
    let s = child(child(root, "startups")?, name)?;
    let f = |k: &str, d: f32| s.get_f32(k).unwrap_or(d);
    let d = Startup::default();
    Some(Startup {
        flicker_rate: (f("flicker_rate_min", d.flicker_rate.0), f("flicker_rate_max", d.flicker_rate.1)),
        // The game reads the quick dimming as a random number between the
        // minimum and the minimum again.
        flicker_quick: f("flicker_quick_min", d.flicker_quick),
        bg_flicker_length: f("bg_flicker_length", d.bg_flicker_length),
        level_number_delay: f("level_number_delay", d.level_number_delay),
        icon_delay: f("icon_delay", d.icon_delay),
        progress_delay: f("progress_delay", d.progress_delay),
    })
}

/// The screen's quad, `width` along +Y and `height` along +Z from the origin.
fn screen_mesh(width: f32, height: f32) -> Mesh {
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[0.0, 0.0, 0.0], [0.0, width, 0.0], [0.0, width, height], [0.0, 0.0, height]],
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[1.0, 0.0, 0.0]; 4])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]])
        .with_inserted_indices(Indices::U16(vec![0, 1, 2, 0, 2, 3]))
}

fn spawn_screen(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    if !me.get("panelname").is_some_and(|p| p.eq_ignore_ascii_case(PANEL)) {
        return;
    }
    let fs = world.resource::<GameFs>().clone();
    let Some(map) = world.get_resource::<CurrentMap>().map(|m| m.name.to_ascii_lowercase()) else { return };
    let Some(board) = load_board(&fs, &map) else {
        debug!("progress sign {e}: no lightboard entry for {map}");
        return;
    };
    debug!("progress sign {e}: {board:?}");
    let size = panel_size(&fs, PANEL).unwrap_or((400, 808));
    let width = me.get_f32("width").unwrap_or(94.0).max(1.0);
    let height = me.get_f32("height").unwrap_or(190.0).max(1.0);
    let image = world.resource_mut::<Assets<Image>>().add(Image::new_fill(
        Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    ));
    // VGUI materials are UnlitGeneric: unlit, scaled by the exposure.
    let material = world.resource_mut::<Assets<WorldMaterial>>().add(WorldMaterial {
        params: portal2_render::source_material::WorldParams {
            color: Vec4::ONE,
            flags: world_flags::UNLIT,
            lightmap_scale: 1.0,
            envmap_saturation: 1.0,
            ..default()
        },
        base: Some(image.clone()),
        cull: Some(Face::Back),
        ..default()
    });
    let mesh = world.resource_mut::<Assets<Mesh>>().add(screen_mesh(width, height));
    // The panel sits a hair in front of the screen brush behind it.
    let quad = world
        .spawn((Name::new("progress sign"), Mesh3d(mesh), MeshMaterial3d(material), Transform::from_xyz(0.1, 0.0, 0.0), Visibility::Hidden))
        .id();
    world.entity_mut(e).add_child(quad);
    world.entity_mut(e).insert(ProgressSign {
        // Screens start active; maps switch the signs off as they spawn.
        active: true,
        board,
        size,
        starting: None,
        last_flicker: 0.0,
        flicker_rate: 0.0,
        flicker_quick: 0.0,
        board_grey: BOARD_GREY,
        dirt_alpha: 255,
        rng: (e.to_bits() as u32).wrapping_mul(2_654_435_761).max(1),
        quad: Some(quad),
        image,
        drawn: None,
    });
}

fn screen_input(world: &mut World, call: &InputCall) -> bool {
    let active = match call.input.as_str() {
        "setactive" => true,
        "setinactive" => false,
        _ => return false,
    };
    if let Some(mut s) = world.get_mut::<ProgressSign>(call.target) {
        s.active = active;
    }
    true
}

impl ProgressSign {
    fn random(&mut self, lo: f32, hi: f32) -> f32 {
        // xorshift32
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        lo + (hi - lo) * (x >> 8) as f32 / (1u32 << 24) as f32
    }

    fn start(&mut self, now: f64) {
        self.starting = Some(now);
        self.last_flicker = now;
        let s = self.board.startup;
        self.flicker_rate = self.random(s.flicker_rate.0, s.flicker_rate.1);
        self.flicker_quick = s.flicker_quick;
    }

    /// Board grey, dirt alpha, number alpha and icon fade (in 255ths) now.
    fn state(&mut self, now: f64) -> (u8, u8, u8, u8) {
        let Some(start) = self.starting else { return (BOARD_GREY, 255, 255, 255) };
        let s = self.board.startup;
        if now > start + s.length() as f64 {
            self.starting = None;
            self.board_grey = BOARD_GREY;
            self.dirt_alpha = 255;
            return (BOARD_GREY, 255, 255, 255);
        }
        if start + (s.bg_flicker_length as f64) < now {
            self.board_grey = BOARD_GREY;
            self.dirt_alpha = 255;
        } else {
            if now >= self.last_flicker + self.flicker_quick as f64 {
                self.board_grey = BOARD_GREY / 4;
                self.dirt_alpha = 63;
            }
            if now >= self.last_flicker + self.flicker_rate as f64 {
                self.board_grey = if self.board_grey == BOARD_GREY { BOARD_GREY / 4 } else { BOARD_GREY / 2 };
                self.dirt_alpha = if self.dirt_alpha != 255 { 127 } else { 63 };
                self.last_flicker = now;
                self.flicker_rate = self.random(s.flicker_rate.0, s.flicker_rate.1);
                self.flicker_quick = s.flicker_quick;
            }
        }
        let number = if start + (s.level_number_delay as f64) >= now { 2 } else { 255 };
        let icons = if start + (s.progress_delay as f64) >= now { 26 } else { 255 };
        (self.board_grey, self.dirt_alpha, number, icons)
    }
}

/// A decoded texture: linear RGB with alpha.
struct Texture {
    width: u32,
    height: u32,
    texels: Vec<[f32; 4]>,
}

impl Texture {
    fn load(fs: &GameFs, material: &str) -> Option<Self> {
        let vmt = portal2_render::materials::load_vmt(fs, &format!("materials/{material}.vmt"))?;
        let path = vmt.texture_path("$basetexture")?;
        let vtf = source_formats::vtf::Vtf::parse(&fs.read(&path)?).ok()?;
        let texels = vtf
            .decode_rgba_f32(0, 0, 0)
            .into_iter()
            .map(|t| [srgb_to_linear(t[0]), srgb_to_linear(t[1]), srgb_to_linear(t[2]), t[3]])
            .collect();
        Some(Self { width: vtf.width, height: vtf.height, texels })
    }

    /// Bilinear sample, repeating.
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let (w, h) = (self.width as f32, self.height as f32);
        let (x, y) = (u * w - 0.5, v * h - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |xi: f32, yi: f32| {
            let xi = (xi as i64).rem_euclid(self.width as i64) as usize;
            let yi = (yi as i64).rem_euclid(self.height as i64) as usize;
            self.texels[yi * self.width as usize + xi]
        };
        let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
        std::array::from_fn(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy)
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// The panel's pixels in linear light, blended as the game's VGUI does.
struct Canvas {
    width: u32,
    height: u32,
    px: Vec<[f32; 3]>,
}

impl Canvas {
    fn blend(&mut self, x: i32, y: i32, rgb: [f32; 3], a: f32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 || a <= 0.0 {
            return;
        }
        let p = &mut self.px[(y as u32 * self.width + x as u32) as usize];
        for i in 0..3 {
            p[i] = rgb[i] * a + p[i] * (1.0 - a);
        }
    }

    /// `DrawTexturedSubRect` with a draw colour.
    fn rect(&mut self, tex: &Texture, (x0, y0, x1, y1): (i32, i32, i32, i32), (u0, v0, u1, v1): (f32, f32, f32, f32), color: [u8; 4]) {
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let tint =
            [srgb_to_linear(color[0] as f32 / 255.0), srgb_to_linear(color[1] as f32 / 255.0), srgb_to_linear(color[2] as f32 / 255.0)];
        let alpha = color[3] as f32 / 255.0;
        for y in y0..y1 {
            let v = v0 + (v1 - v0) * ((y - y0) as f32 + 0.5) / (y1 - y0) as f32;
            for x in x0..x1 {
                let u = u0 + (u1 - u0) * ((x - x0) as f32 + 0.5) / (x1 - x0) as f32;
                let t = tex.sample(u, v);
                self.blend(x, y, [t[0] * tint[0], t[1] * tint[1], t[2] * tint[2]], t[3] * alpha);
            }
        }
    }

    fn text(&mut self, font: &mut RasterFont, text: &str, x: i32, y: i32, color: [u8; 4]) {
        let tint =
            [srgb_to_linear(color[0] as f32 / 255.0), srgb_to_linear(color[1] as f32 / 255.0), srgb_to_linear(color[2] as f32 / 255.0)];
        let alpha = color[3] as f32 / 255.0;
        let mut pen = x;
        for ch in text.chars() {
            let abc = font.abc(ch);
            let (w, h, cell) = font.rasterize(ch);
            for cy in 0..h {
                for cx in 0..w {
                    let i = ((cy * w + cx) * 4) as usize;
                    let a = cell[i + 3] as f32 / 255.0 * alpha;
                    self.blend(pen + abc.a + cx, y + cy, tint, a);
                }
            }
            pen += abc.advance();
        }
    }
}

/// The sign's textures, loaded once.
struct SignTextures {
    board: Option<Texture>,
    numbers: Option<Texture>,
    bar: Option<Texture>,
    icons: Option<Texture>,
    dirt: [Option<Texture>; 3],
}

impl SignTextures {
    fn load(fs: &GameFs) -> Self {
        Self {
            board: Texture::load(fs, "vgui/screens/vgui_coop_progress_board"),
            numbers: Texture::load(fs, "vgui/screens/vgui_coop_progress_board_numbers"),
            bar: Texture::load(fs, "vgui/screens/vgui_coop_progress_board_bar"),
            icons: Texture::load(fs, "vgui/screens/p2_lightboard_vgui"),
            dirt: std::array::from_fn(|i| Texture::load(fs, &format!("vgui/elevator_video_overlay{}", i + 1))),
        }
    }
}

/// Paints the panel (`CVGUI_SP_ProgressSignScreen`): the board, the chamber
/// number's two digits from the number sheet, the progress bar, the icons,
/// the dirt and the "NN/NN" label.
fn draw(
    sign: &ProgressSign,
    tex: &SignTextures,
    font: &mut RasterFont,
    (grey, dirt_alpha, number_alpha, icon_fade): (u8, u8, u8, u8),
) -> Vec<u8> {
    let (w, h) = sign.size;
    let mut c = Canvas { width: w, height: h, px: vec![[0.0; 3]; (w * h) as usize] };
    let board = &sign.board;
    if let Some(t) = &tex.board {
        c.rect(t, (0, 0, w as i32, h as i32), (0.0, 0.0, 1.0, 1.0), [grey, grey, grey, 255]);
    }
    // Digits: a sixth of the sheet wide and half of it tall, 0-5 on the top
    // row and 6-9 below; 40% of the panel's width tall, a third as wide.
    if let Some(t) = &tex.numbers {
        let tall = (w as f32 * 0.4) as i32;
        let wide = tall / 3;
        let level = board.level.max(0);
        for (i, digit) in [(level / 10) % 10, level % 10].into_iter().enumerate() {
            let (col, row) = if digit > 5 { (digit - 6, 0.5) } else { (digit, 0.0) };
            let x = 70 + wide * i as i32;
            let u = col as f32 / 6.0;
            c.rect(t, (x, tall / 4, x + wide, tall / 4 + tall), (u, row, u + 1.0 / 6.0, row + 0.5), [255, 255, 255, number_alpha]);
        }
    }
    if board.total > 0
        && let Some(t) = &tex.bar
    {
        let span = w as f32 - 162.0;
        let share = if board.level > 0 { board.level as f32 / board.total as f32 } else { 1.0 };
        c.rect(t, (81, 461, (span * share + 81.0) as i32, 493), (0.0, 0.0, span / 32.0 * share, 1.0), [255, 255, 255, number_alpha]);
    }
    if let Some(t) = &tex.icons {
        let (mut x, mut y) = (81, 577);
        for (i, icon) in board.icons.iter().enumerate() {
            if i == 5 {
                x = 81;
                y += 63;
            }
            let Some((cell, lit)) = icon else { continue };
            let a = if *lit { 255.0 } else { ICON_DIM_ALPHA } * icon_fade as f32 / 255.0;
            let (u, v) = ((cell % 5) as f32 * 0.2, (cell / 5) as f32 * 0.2);
            c.rect(t, (x, y, x + 53, y + 53), (u, v, u + 0.2, v + 0.2), [0, 0, 0, a as u8]);
            x += 63;
        }
    }
    if let Some(Some(t)) = board.dirt.map(|d| &tex.dirt[d]) {
        c.rect(t, (0, 0, w as i32, h as i32), (0.0, 0.0, 1.0, 1.0), [255, 255, 255, dirt_alpha]);
    }
    // `LevelProgressLabel`: at (81, 386), 100 tall, text centred vertically.
    let text = format!("{:02}/{:02}", board.level, board.total);
    let y = 386 + (100 - font.metrics().height) / 2;
    c.text(font, &text, 81, y, [0, 0, 0, number_alpha]);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for p in &c.px {
        out.extend([p[0], p[1], p[2]].map(|v| (linear_to_srgb(v) * 255.0 + 0.5) as u8));
        out.push(255);
    }
    out
}

#[allow(clippy::type_complexity)]
fn paint_signs(
    clock: Res<GameClock>,
    fs: Res<GameFs>,
    schemes: Res<Schemes>,
    mut signs: Query<&mut ProgressSign>,
    mut visibility: Query<&mut Visibility>,
    mut images: ResMut<Assets<Image>>,
    mut textures: Local<Option<SignTextures>>,
    mut font: Local<Option<RasterFont>>,
    mut was_active: Local<std::collections::HashMap<Entity, bool>>,
    entities: Query<Entity, With<ProgressSign>>,
) {
    if signs.is_empty() || schemes.symbols.is_none() {
        return;
    }
    let tex = textures.get_or_insert_with(|| SignTextures::load(&fs));
    let Some(spec) = schemes.basemodui.font_spec("CoopLevelProgressFont_Small", 480, false) else { return };
    let font = font.get_or_insert_with(|| RasterFont::new(spec));
    let now = clock.curtime;
    for e in &entities {
        let Ok(mut sign) = signs.get_mut(e) else { continue };
        let before = was_active.insert(e, sign.active).unwrap_or(false);
        if sign.active && !before {
            sign.start(now);
        }
        if let Some(mut v) = sign.quad.and_then(|q| visibility.get_mut(q).ok()) {
            let want = if sign.active { Visibility::Inherited } else { Visibility::Hidden };
            if *v != want {
                *v = want;
            }
        }
        if !sign.active {
            continue;
        }
        let state = sign.state(now);
        if sign.drawn == Some(state) {
            continue;
        }
        debug!("progress sign {e}: drawing {state:?}");
        let pixels = draw(&sign, tex, font, state);
        if let Some(mut image) = images.get_mut(&sign.image) {
            image.data = Some(pixels);
        }
        sign.drawn = Some(state);
    }
}
