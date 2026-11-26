//! Paint (gels) in play: `info_paint_sprayer` blobs and their splats,
//! paint bombs, painted props and the gel powers' rules. The paint on the
//! world's surfaces is the world crate's paint map
//! (`portal2_world::paint_map`). The rules follow the game's
//! (docs/re/paint.md).

pub mod blobs;
pub mod bomb;
pub mod power;
pub mod props;
pub mod surface;

use bevy::math::Affine3A;
use bevy::prelude::*;

use portal2_core::entity::MapEntity;
use portal2_world::paint_map::{PaintMap, PaintType};

/// The paint around a spot: the world's and that of the brush entities
/// near it (a func_brush floor carries its own paint, as in the engine).
pub struct PaintedSurfaces<'a> {
    pub map: &'a PaintMap,
    /// Brush model and the transform into its space.
    brushes: Vec<(usize, Affine3A)>,
}

impl<'a> PaintedSurfaces<'a> {
    /// Keeps the brush entities with paintable faces within `reach` of `center`.
    pub fn near<'e>(
        map: &'a PaintMap,
        entities: impl IntoIterator<Item = (&'e MapEntity, &'e GlobalTransform)>,
        center: Vec3,
        reach: f32,
    ) -> Self {
        let mut brushes = Vec::new();
        for (me, gt) in entities {
            let Some(model) = me.brush_model().filter(|&m| m > 0) else { continue };
            let Some(m) = map.models.get(model).filter(|m| !m.faces.is_empty()) else { continue };
            let to_local = gt.affine().inverse();
            let local = to_local.transform_point3(center);
            if local.clamp(m.mins, m.maxs).distance_squared(local) <= reach * reach {
                brushes.push((model, to_local));
            }
        }
        Self { map, brushes }
    }

    /// The paint type most luxels under the sphere carry, over the world
    /// and the nearby brush entities (see `PaintMap::query`).
    pub fn dominant(&self, center: Vec3, radius: f32, min_amount: f32) -> Option<PaintType> {
        let mut counts = self.map.query(0, center, radius, min_amount);
        for &(model, to_local) in &self.brushes {
            let more = self.map.query(model, to_local.transform_point3(center), radius, min_amount);
            for (n, m) in counts.iter_mut().zip(more) {
                *n += m;
            }
        }
        let (i, &n) = counts.iter().enumerate().take(4).max_by_key(|(_, n)| **n)?;
        if n == 0 { None } else { PaintType::from_index(i as i32) }
    }
}

pub struct PaintPlugin;

impl Plugin for PaintPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((blobs::BlobPlugin, bomb::PaintBombPlugin, props::PaintedPropsPlugin));
    }
}
