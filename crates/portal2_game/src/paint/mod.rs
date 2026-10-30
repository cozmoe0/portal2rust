//! Paint (gels) in play: `info_paint_sprayer` blobs and their splats,
//! paint bombs, painted props and the gel powers' rules. The paint on the
//! world's surfaces is the world crate's paint map
//! (`portal2_world::paint_map`). The rules follow the game's
//! (docs/re/paint.md).

pub mod blobs;
pub mod bomb;
pub mod power;
pub mod props;

use bevy::prelude::*;

pub struct PaintPlugin;

impl Plugin for PaintPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((blobs::BlobPlugin, bomb::PaintBombPlugin, props::PaintedPropsPlugin));
    }
}
