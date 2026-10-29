//! The in-game HUD: elements placed by `scripts/hudlayout.res` and drawn
//! with the interface painter.

pub mod credits;
pub mod crosshair;

use bevy::prelude::*;
use source_formats::kv::KeyValues;

use crate::console::Console;
use crate::vfs::GameFs;
use crate::vgui::layout::{self, Rect, ResFile};
use crate::vgui::{Schemes, Surface};

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HudLayout>().add_systems(First, load_layout.after(crate::vgui::TrackScreen));
        app.world_mut().resource_mut::<Console>().register_cvar("cl_drawhud", "1", 0);
        crosshair::build(app);
        credits::build(app);
    }
}

/// `scripts/hudlayout.res`, reloaded with the schemes.
#[derive(Resource, Default)]
pub struct HudLayout {
    file: ResFile,
    generation: u32,
}

impl HudLayout {
    pub fn settings(&self, element: &str) -> Option<&KeyValues> {
        self.file.get(element)
    }

    /// The element's bounds on the screen (HUD elements are proportional
    /// and children of the full-screen HUD panel).
    pub fn rect(&self, element: &str, surface: &Surface, default: Rect) -> Rect {
        let screen = Rect::new(0, 0, surface.wide, surface.tall);
        match self.settings(element) {
            Some(s) => layout::apply(s, default, (surface.wide, surface.tall), screen, true),
            None => default,
        }
    }

    pub fn f32(&self, element: &str, key: &str, default: f32) -> f32 {
        self.settings(element).and_then(|s| s.get_f32(key)).unwrap_or(default)
    }
}

fn load_layout(mut hud: ResMut<HudLayout>, schemes: Res<Schemes>, fs: Res<GameFs>) {
    if hud.generation == schemes.generation {
        return;
    }
    let Some(symbols) = schemes.symbols else { return };
    hud.file = ResFile::load(&fs, "scripts/hudlayout.res", &symbols).unwrap_or_default();
    hud.generation = schemes.generation;
}

/// Whether HUD elements draw this frame (`cl_drawhud`).
pub fn visible(console: &Console) -> bool {
    console.bool("cl_drawhud")
}
