//! The portal gun crosshair (`CHUDQuickInfo`) and the centre dot.
//!
//! Two brackets from `sprites/hud/portal_crosshairs` (the `portal_crosshair_*`
//! rectangles of `scripts/mod_textures.txt`), drawn at their own size: the
//! left one for the first portal 6 pixels left of and 10 above the screen
//! centre, the right one for the second portal 6 right and 10 below. A
//! bracket is filled once its portal is placed. Portal colours are
//! brightened (each channel moves a quarter of the way to white, at least
//! 5); empty brackets draw at alpha 128, filled ones at 255 less the screen
//! fade.

use bevy::prelude::*;

use crate::portals::Portals;
use crate::portals::gun::PortalGun;
use crate::screen::ScreenFade;
use portal2_core::console::Console;
use portal2_ui::vgui::{Surface, VguiPaint, layer};

pub fn build(app: &mut App) {
    app.add_systems(PostUpdate, paint.in_set(VguiPaint));
    app.world_mut().resource_mut::<Console>().register_cvar("hud_quickinfo", "1", portal2_core::console::cvars::flags::ARCHIVE);
}

/// Pixel rectangles in the sprite sheet.
const LEFT_INVALID: [u32; 4] = [2, 0, 44, 64];
const RIGHT_INVALID: [u32; 4] = [50, 0, 44, 64];
const LEFT_VALID: [u32; 4] = [98, 0, 44, 64];
const RIGHT_VALID: [u32; 4] = [146, 0, 44, 64];
const OFFSET: (i32, i32) = (6, 10);
const PORTAL_COLORS: [[u8; 3]; 2] = [[64, 160, 255], [255, 160, 32]];

fn brighten(c: u8) -> u8 {
    if c < 5 { 5 } else { ((1.0 - c as f32 / 255.0) * 64.0 + c as f32).min(255.0) as u8 }
}

#[allow(clippy::too_many_arguments)]
fn paint(
    console: Res<Console>,
    gun: Option<Res<PortalGun>>,
    portals: Option<Res<Portals>>,
    fade: Res<ScreenFade>,
    clock: Res<portal2_core::clock::GameClock>,
    ui: Res<crate::gameui::GameUi>,
    map: Option<Res<portal2_world::CurrentMap>>,
    mut surface: ResMut<Surface>,
) {
    if map.is_none() || ui.active() || !portal2_ui::hud::visible(&console) {
        return;
    }
    let (cx, cy) = (surface.wide / 2, surface.tall / 2);
    surface.set_layer(layer::HUD);
    surface.fill(cx, cy, cx + 1, cy + 1, [255, 255, 255, 255]);
    let Some(gun) = gun else { return };
    if !console.bool("hud_quickinfo") || !gun.can_fire.iter().any(|f| *f) {
        return;
    }
    let Some(sheet) = surface.image("sprites/hud/portal_crosshairs") else { return };
    let fade_alpha = fade.alpha(clock.curtime) * 255.0;
    let filled_alpha = (255.0 - fade_alpha).clamp(0.0, 255.0) as u8;
    let placed = |slot: usize| portals.as_ref().is_some_and(|p| p.entities[slot].is_some());
    for slot in 0..2 {
        if !gun.can_fire[slot] {
            continue;
        }
        let c = PORTAL_COLORS[slot].map(brighten);
        let alpha = if placed(slot) { filled_alpha } else { 128 };
        let rect = match (slot, placed(slot)) {
            (0, true) => LEFT_VALID,
            (0, false) => LEFT_INVALID,
            (_, true) => RIGHT_VALID,
            (_, false) => RIGHT_INVALID,
        };
        let (w, h) = (rect[2] as i32, rect[3] as i32);
        let (dx, dy) = if slot == 0 { (-OFFSET.0, -OFFSET.1) } else { OFFSET };
        let (x, y) = (cx + dx - w / 2, cy + dy - h / 2);
        let (sw, sh) = (sheet.width as f32, sheet.height as f32);
        let uv = [rect[0] as f32 / sw, rect[1] as f32 / sh, (rect[0] + rect[2]) as f32 / sw, (rect[1] + rect[3]) as f32 / sh];
        surface.draw_image_uv(&sheet, x as f32, y as f32, (x + w) as f32, (y + h) as f32, uv, [c[0], c[1], c[2], alpha]);
    }
}
