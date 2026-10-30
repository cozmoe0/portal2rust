//! Light styles: each style has a pattern of letters stepped through at ten
//! letters a second, `a` dark and `m` normal (`docs/re/world_visuals.md`,
//! "Light styles"). Style 0 is the normal light; the map compiler gives every
//! named light its own style from 32 up, so switching it on, off or to a
//! flicker pattern scales its share of the lightmaps (and of model lighting).
//! Faces lit by such lights keep one lightmap layer per style; their atlas
//! texels are composed again whenever a style's brightness changes.

use bevy::prelude::*;

use crate::bsp_mesh::StyledFace;
use crate::paint_map::PaintRect;
use crate::paint_map::upload::PaintUploads;

/// Number of light styles the engine keeps.
pub const MAX_STYLES: usize = 64;

pub struct LightStylePlugin;

impl Plugin for LightStylePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LightStyles>().add_systems(PostUpdate, compose_styled_lightmap);
    }
}

#[derive(Resource)]
pub struct LightStyles {
    patterns: Vec<String>,
    /// Bumped whenever any style's brightness changes, so lit models can
    /// redo their lighting.
    pub generation: u32,
    values: [f32; MAX_STYLES],
}

impl Default for LightStyles {
    fn default() -> Self {
        Self { patterns: vec!["m".to_owned(); MAX_STYLES], generation: 0, values: [1.0; MAX_STYLES] }
    }
}

impl LightStyles {
    pub fn set(&mut self, style: usize, pattern: &str) {
        if let Some(p) = self.patterns.get_mut(style)
            && p != pattern
        {
            *p = pattern.to_owned();
        }
    }

    /// Brightness of a style (1 for `m`, the engine's 264 "normal").
    pub fn value(&self, style: i32) -> f32 {
        usize::try_from(style).ok().and_then(|s| self.values.get(s)).copied().unwrap_or(0.0)
    }

    /// Steps every style's pattern to `time` (seconds); true when any
    /// brightness changed.
    fn advance(&mut self, time: f64) -> bool {
        let step = (time * 10.0).max(0.0) as usize;
        let mut changed = false;
        for (style, pattern) in self.patterns.iter().enumerate() {
            let bytes = pattern.as_bytes();
            let v = if bytes.is_empty() { 1.0 } else { letter_value(bytes[step % bytes.len()]) };
            if (self.values[style] - v).abs() > 1e-6 {
                self.values[style] = v;
                changed = true;
            }
        }
        if changed {
            self.generation = self.generation.wrapping_add(1);
        }
        changed
    }
}

fn letter_value(c: u8) -> f32 {
    (c.saturating_sub(b'a')) as f32 * 22.0 / 264.0
}

/// The current map's faces with more than one light style.
#[derive(Resource)]
pub struct StyledLightmap {
    pub image: AssetId<Image>,
    pub faces: Vec<StyledFace>,
    /// Style values the atlas was last composed with.
    applied: [f32; MAX_STYLES],
}

impl StyledLightmap {
    pub fn new(image: AssetId<Image>, faces: Vec<StyledFace>) -> Self {
        // The atlas starts with only style 0 lit.
        let mut applied = [0.0; MAX_STYLES];
        applied[0] = 1.0;
        Self { image, faces, applied }
    }
}

fn compose_styled_lightmap(
    clock: Res<portal2_core::clock::GameClock>,
    mut styles: ResMut<LightStyles>,
    lightmap: Option<ResMut<StyledLightmap>>,
    mut uploads: ResMut<PaintUploads>,
) {
    styles.advance(clock.curtime);
    let Some(mut lightmap) = lightmap else { return };
    let changed: Vec<usize> = (0..MAX_STYLES).filter(|&s| (lightmap.applied[s] - styles.values[s]).abs() > 1e-6).collect();
    if changed.is_empty() {
        return;
    }
    let mut rects = Vec::new();
    for face in &lightmap.faces {
        if !face.layers.iter().any(|(st, _)| changed.contains(&(*st as usize))) {
            continue;
        }
        let n = (face.width * face.height) as usize;
        let mut data = Vec::with_capacity(n * 8);
        for i in 0..n {
            let mut c = [0.0f32; 3];
            for (st, layer) in &face.layers {
                let v = styles.value(*st as i32);
                if v == 0.0 {
                    continue;
                }
                for k in 0..3 {
                    c[k] += layer[i][k] * v;
                }
            }
            for x in [c[0], c[1], c[2], 1.0] {
                data.extend_from_slice(&half::f16::from_f32(x).to_le_bytes());
            }
        }
        rects.push(PaintRect { image: lightmap.image, x: face.x, y: face.y, width: face.width, height: face.height, texel_bytes: 8, data });
    }
    for s in changed {
        lightmap.applied[s] = styles.values[s];
    }
    uploads.push(rects);
}
