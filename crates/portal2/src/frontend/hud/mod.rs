//! The in-game HUD elements: the portal crosshair, the game instructor's
//! hints and the ending credits, placed by `scripts/hudlayout.res`
//! (`portal2_ui::hud`) and drawn with the interface painter.

pub mod credits;
pub mod crosshair;
pub mod instructor;

use bevy::prelude::*;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        crosshair::build(app);
        credits::build(app);
        instructor::build(app);
    }
}
