pub mod fog;
pub mod materials;
pub mod source_material;
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
        app.add_plugins((
            MaterialPlugin::<source_material::WorldMaterial>::default(),
            MaterialPlugin::<source_material::ModelMaterial>::default(),
        ))
        .init_resource::<textures::TextureCache>()
        .init_resource::<materials::MaterialCache>()
        // World lighting comes entirely from baked lightmaps and ambient cubes.
        .insert_resource(GlobalAmbientLight { brightness: 0.0, ..default() })
        .insert_resource(ClearColor(Color::BLACK));
    }
}
