//! Drawing Source content with Bevy: the game's materials (`.vmt`) turned
//! into shader parameters ([`materials`], [`source_material`]), textures
//! (`.vtf`) uploaded as they are ([`textures`]), studio models with their
//! animation and lighting ([`models`]), and the engine's WESL shaders, which
//! every crate's materials load from `embedded://portal2_render/shaders/`.

pub mod fx;
pub mod materials;
pub mod models;
pub mod source_material;
pub mod surfaces;
pub mod textures;

use bevy::asset::embedded_asset;
use bevy::prelude::*;

pub struct RenderSetupPlugin;

impl Plugin for RenderSetupPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/world.wesl");
        embedded_asset!(app, "shaders/model.wesl");
        embedded_asset!(app, "shaders/portal.wesl");
        embedded_asset!(app, "shaders/effect.wesl");
        embedded_asset!(app, "shaders/refract.wesl");
        embedded_asset!(app, "shaders/sky.wesl");
        embedded_asset!(app, "shaders/projected.wesl");
        embedded_asset!(app, "shaders/rope.wesl");
        embedded_asset!(app, "shaders/sprite.wesl");
        embedded_asset!(app, "shaders/beam.wesl");
        embedded_asset!(app, "shaders/laser.wesl");
        embedded_asset!(app, "shaders/tonemap_histogram.wesl");
        embedded_asset!(app, "shaders/particle.wesl");
        embedded_asset!(app, "shaders/fx.wesl");
        embedded_asset!(app, "shaders/fx_post.wesl");
        embedded_asset!(app, "shaders/fx_ao.wesl");
        embedded_asset!(app, "shaders/fx_volumetric.wesl");
        app.add_plugins((
            MaterialPlugin::<source_material::WorldMaterial>::default(),
            MaterialPlugin::<source_material::ModelMaterial>::default(),
            MaterialPlugin::<source_material::RefractMaterial>::default(),
        ))
        .init_resource::<textures::TextureCache>()
        .init_resource::<materials::MaterialCache>()
        // World lighting comes entirely from baked lightmaps and ambient cubes.
        .insert_resource(GlobalAmbientLight { brightness: 0.0, ..default() })
        .insert_resource(ClearColor(Color::BLACK));
    }
}
