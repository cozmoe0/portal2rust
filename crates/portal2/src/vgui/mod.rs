//! The game's 2D interface layer: schemes, fonts, layout files and the
//! painter that HUD, menus and the console draw with.

pub mod font;
pub mod image;
pub mod layout;
pub mod render;
pub mod scheme;
pub mod surface;
pub mod widgets;

use bevy::asset::embedded_asset;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{Exposure, Hdr};
use bevy::prelude::*;
use bevy::render::view::Tonemapping;
use bevy::window::PrimaryWindow;

use crate::vfs::GameFs;
pub use scheme::{Scheme, Symbols};
pub use surface::{FontId, ImageRef, Surface, layer};

/// Render layer nothing is on: the compositing camera draws no meshes.
const COMPOSITE_LAYER: usize = 31;

/// Screen size and scheme tracking (First).
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TrackScreen;

/// Interface painting happens in this set (PostUpdate); the frame's
/// geometry is finished after it.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct VguiPaint;

pub struct VguiPlugin;

impl Plugin for VguiPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/vgui.wesl");
        app.add_plugins(render::VguiRenderPlugin)
            .init_resource::<Surface>()
            .init_resource::<Schemes>()
            .add_systems(Startup, (setup_surface, spawn_composite_camera))
            .add_systems(First, track_screen.in_set(TrackScreen))
            .configure_sets(PostUpdate, VguiPaint.before(finish_frame))
            .add_systems(PostUpdate, (finish_frame, debug_paint.in_set(VguiPaint)));
    }
}

/// The schemes interface code uses, loaded for the current screen.
#[derive(Resource, Default)]
pub struct Schemes {
    pub symbols: Option<Symbols>,
    /// `resource/basemodui_scheme.res`: menus and most of the HUD.
    pub basemodui: Scheme,
    /// `resource/clientscheme.res`.
    pub client: Scheme,
    /// `resource/sourcescheme.res`: the console and engine dialogs.
    pub source: Scheme,
    /// Bumped whenever the schemes are reloaded (screen size changes), so
    /// panels redo their layout.
    pub generation: u32,
    fonts_registered: bool,
}

fn setup_surface(mut surface: ResMut<Surface>, fs: Res<GameFs>) {
    surface.set_filesystem(fs.0.clone());
}

fn spawn_composite_camera(mut commands: Commands) {
    commands.spawn((
        Name::new("interface camera"),
        render::VguiComposite,
        Camera3d::default(),
        Camera { order: 100, clear_color: ClearColorConfig::None, ..default() },
        Hdr,
        Tonemapping::None,
        bevy::render::view::DebandDither::Disabled,
        Exposure::default(),
        // Matches the main camera so the two share the view's textures.
        Msaa::Sample4,
        RenderLayers::layer(COMPOSITE_LAYER),
        Transform::default(),
    ));
}

/// Keeps the surface size and the schemes in step with the window.
fn track_screen(
    window: Query<&Window, With<PrimaryWindow>>,
    mut surface: ResMut<Surface>,
    mut schemes: ResMut<Schemes>,
    fs: Res<GameFs>,
) {
    let Ok(window) = window.single() else { return };
    let (w, h) = (window.physical_width().max(1) as i32, window.physical_height().max(1) as i32);
    surface.wide = w;
    surface.tall = h;
    let symbols = Symbols::for_screen(w, h);
    if schemes.symbols == Some(symbols) {
        return;
    }
    schemes.basemodui = Scheme::load(&fs, "resource/basemodui_scheme.res", &symbols).unwrap_or_default();
    schemes.client = Scheme::load(&fs, "resource/clientscheme.res", &symbols).unwrap_or_default();
    schemes.source = Scheme::load(&fs, "resource/sourcescheme.res", &symbols).unwrap_or_default();
    if !schemes.fonts_registered {
        schemes.fonts_registered = true;
        let files: Vec<String> = [&schemes.basemodui, &schemes.client, &schemes.source]
            .iter()
            .flat_map(|s| s.custom_font_files.iter().cloned())
            .collect();
        let mut seen = std::collections::HashSet::new();
        for file in files {
            if !seen.insert(file.to_ascii_lowercase()) {
                continue;
            }
            let Some(data) = fs.read(&file) else { continue };
            let data = if file.to_ascii_lowercase().ends_with(".vfont") {
                match source_formats::vfont::decode(&data) {
                    Some(d) => d,
                    None => continue,
                }
            } else {
                data
            };
            if !font::register_font_data(data) {
                warn!("vgui: font file {file} could not be added");
            }
        }
    }
    schemes.symbols = Some(symbols);
    schemes.generation += 1;
}

fn finish_frame(mut surface: ResMut<Surface>, mut frame: ResMut<render::VguiFrame>, mut images: ResMut<Assets<Image>>) {
    surface.finish(&mut frame, &mut images);
}

/// `P2R_VGUI_TEST=1`: paints a font and colour sample for checking the
/// interface renderer.
pub fn debug_paint(mut surface: ResMut<Surface>, schemes: Res<Schemes>) {
    if std::env::var_os("P2R_VGUI_TEST").is_none() {
        return;
    }
    let tall = surface.tall;
    surface.set_layer(layer::GAMEUI);
    surface.fill(40, 40, 700, 400, [0, 0, 0, 128]);
    let mut y = 50;
    for name in ["MainMenuItem", "DialogTitle", "DialogMenuItem", "CloseCaption_Normal", "InGameChapterTitle", "ButtonText_NC"] {
        let Some(spec) = schemes.basemodui.font_spec(name, tall, true) else { continue };
        let font = surface.font(spec);
        surface.draw_text(font, 60, y, &format!("{name}: The quick brown fox 0123"), [255, 255, 255, 255]);
        y += surface.font_tall(font) + 4;
    }
    if let Some(logo) = surface.image("vgui/portal2logo") {
        surface.draw_image(&logo, 60, y, 60 + 450, y + 112, [255, 255, 255, 255]);
    }
}
