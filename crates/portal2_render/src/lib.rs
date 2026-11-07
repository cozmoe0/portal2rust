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
        embedded_asset!(app, "shaders/fx_ssr.wesl");
        app.add_plugins((
            MaterialPlugin::<source_material::WorldMaterial>::default(),
            MaterialPlugin::<source_material::ModelMaterial>::default(),
            MaterialPlugin::<source_material::RefractMaterial>::default(),
        ))
        .init_resource::<textures::TextureCache>()
        .init_resource::<materials::MaterialCache>()
        // World lighting comes entirely from baked lightmaps and ambient cubes.
        .insert_resource(GlobalAmbientLight { brightness: 0.0, ..default() })
        .insert_resource(ClearColor(Color::BLACK))
        .add_systems(PostUpdate, single_cluster);
    }

    fn finish(&self, app: &mut App) {
        // GPUs without BC formats (phones and tablets in the browser) get
        // the block-compressed textures expanded to RGBA8.
        if let Some(support) = app.world().get_resource::<bevy::image::CompressedImageFormatSupport>() {
            let bc = support.0.contains(bevy::image::CompressedImageFormats::BC);
            textures::set_bc_supported(bc);
            if !bc {
                warn!("the GPU has no BC texture formats; textures are expanded to RGBA8");
            }
        }
        // Nothing is lit by Bevy's clustered lights, decals or probes (the
        // game's light comes from lightmaps, ambient cubes and its own
        // buffers), so every view's clustering is wasted: GPU clustering runs
        // several passes and a readback per view each frame (the portals'
        // views included), the CPU fallback does nothing without objects.
        if let Some(mut settings) = app.world_mut().get_resource_mut::<bevy::light::cluster::GlobalClusterSettings>() {
            settings.gpu_clustering = None;
        }
    }
}

/// Nothing is lit by Bevy's clustered lights (see `finish`), yet every view
/// would still build a grid of thousands of clusters on the CPU each frame:
/// one cluster per view does. (`ClusterConfig::None` leaves the views'
/// cluster bindings out, which the materials' layouts expect.)
fn single_cluster(mut cameras: Query<&mut bevy::light::cluster::ClusterConfig, Added<Camera3d>>) {
    for mut config in &mut cameras {
        *config = bevy::light::cluster::ClusterConfig::Single;
    }
}
