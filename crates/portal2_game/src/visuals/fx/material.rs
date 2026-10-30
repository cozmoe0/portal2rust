//! The overhaul's side of the world and model materials: the storage buffer
//! they all bind (switches, time, lights; see `shaders/fx.wesl`), and the
//! `fx` flag that picks their shader variant, kept in step with the preset.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::storage::ShaderBuffer;
use portal2_render::fx::{BUFFER_FLOATS, FX, FX_SHADER, FxHandles};
use portal2_render::source_material::{ModelMaterial, WorldMaterial};

use super::FxSettings;
use portal2_core::console::ConsoleSet;
use portal2_core::sets::LoadMapSet;

/// Overall strength of the overhaul lights' highlights.
const SPECULAR_SCALE: f32 = 1.0;
/// Strength of fixture highlights on glossy surfaces.
const FIXTURE_SPECULAR_SCALE: f32 = 0.6;

/// The buffer's contents, filled by the overhaul's systems each frame and
/// uploaded once in `upload`.
#[derive(Resource)]
pub struct FxBuffer {
    pub handle: Handle<ShaderBuffer>,
    pub data: Vec<f32>,
}

impl FxBuffer {
    pub fn header_mut(&mut self) -> &mut [f32] {
        &mut self.data[..portal2_render::fx::HEADER_FLOATS]
    }
}

/// Keeps the shared WESL module loaded so the shaders importing it resolve.
#[derive(Resource)]
#[allow(dead_code)]
struct FxShaderModule(Handle<Shader>);

/// Fills the buffer (before `upload`).
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FxFillSet;

pub struct FxMaterialPlugin;

impl Plugin for FxMaterialPlugin {
    fn build(&self, app: &mut App) {
        let handle = app
            .world_mut()
            .resource_mut::<Assets<ShaderBuffer>>()
            .add(ShaderBuffer::new(vec![0f32; BUFFER_FLOATS], RenderAssetUsages::default()));
        let shadows = {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            std::array::from_fn(|_| images.add(crate::visuals::projected::shadow_image()))
        };
        let _ = FX.set(FxHandles { buffer: handle.clone(), shadows });
        let settings = app.world().resource::<FxSettings>().clone();
        portal2_render::fx::set_fx_enabled(settings.enabled());
        app.insert_resource(FxBuffer { handle, data: vec![0.0; BUFFER_FLOATS] })
            .add_systems(Startup, load_shader_module)
            .add_systems(PreUpdate, sync_variant.after(super::update_settings).after(ConsoleSet).before(LoadMapSet))
            .add_systems(PostUpdate, (fill_header.in_set(FxFillSet), upload.after(FxFillSet)));
    }
}

fn load_shader_module(mut commands: Commands, assets: Res<AssetServer>) {
    commands.insert_resource(FxShaderModule(assets.load(FX_SHADER)));
}

/// Moves every world and model material to the overhaul's shader variant,
/// or back, when the overhaul turns on or off.
fn sync_variant(settings: Res<FxSettings>, mut world: ResMut<Assets<WorldMaterial>>, mut model: ResMut<Assets<ModelMaterial>>) {
    let on = settings.enabled();
    if on == portal2_render::fx::fx_enabled() {
        return;
    }
    portal2_render::fx::set_fx_enabled(on);
    for (_, m) in world.iter_mut() {
        m.fx = on;
    }
    for (_, m) in model.iter_mut() {
        m.fx = on;
    }
}

fn fill_header(time: Res<Time>, settings: Res<FxSettings>, mut buffer: ResMut<FxBuffer>) {
    let h = buffer.header_mut();
    h[0] = settings.features as f32;
    h[1] = time.elapsed_secs_wrapped();
    h[3] = settings.emissive_boost;
    h[4] = settings.wetness;
    h[5] = SPECULAR_SCALE;
    h[6] = FIXTURE_SPECULAR_SCALE;
}

fn upload(settings: Res<FxSettings>, buffer: Res<FxBuffer>, mut buffers: ResMut<Assets<ShaderBuffer>>) {
    // With the overhaul off no shader reads the buffer.
    if !settings.enabled() && !settings.is_changed() {
        return;
    }
    if let Some(mut b) = buffers.get_mut(&buffer.handle) {
        *b = ShaderBuffer::new(buffer.data.clone(), RenderAssetUsages::default());
    }
}
