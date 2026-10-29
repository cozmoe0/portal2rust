//! Sprites: `env_sprite` (also `env_glow`, `env_sprite_oriented`) and the
//! halos of spotlights.
//!
//! A sprite is its material's texture on a quad `scale` × the texture's
//! size, centred on the entity and turned as `$spriteorientation` says. The
//! colour is `rendercolor` × `renderamt` / 255 × `HDRColorScale`. Render
//! mode 5 (additive) is drawn depth tested; glows (modes 3 and 9) are drawn
//! over everything, scaled by how much of a `GlowProxySize` area around the
//! origin the view can see and faded by distance (full brightness within
//! 1200 units, then by 1200² / distance²); mode 3 also keeps its size on
//! screen (scale × distance / 200). Additive sprites fog toward black. A
//! named sprite starts hidden unless spawn flag 1 is set; unnamed ones are
//! always shown.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, CompareFunction, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;
use serde::{Deserialize, Serialize};

use crate::camera::MainCamera;
use crate::collision::Physics;
use crate::entities::io::{InputCall, InputHandlers};
use crate::entities::{MapEntity, SpawnHandlers};
use crate::render::materials::load_vmt;
use crate::render::textures::TextureCache;
use crate::vfs::GameFs;

pub const SPRITE_SHADER: &str = "embedded://portal2/render/shaders/sprite.wesl";

const MODE_GLOW: u8 = 3;
const MODE_TRANS_ADD: u8 = 5;
const MODE_WORLD_GLOW: u8 = 9;

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct SpriteParams {
    pub color: Vec4,
    pub shape: Vec4,
    pub state: Vec4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SpriteKey {
    /// Glows are not depth tested.
    pub no_depth_test: bool,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
#[bind_group_data(SpriteKey)]
pub struct SpriteMaterial {
    #[uniform(0)]
    pub params: SpriteParams,
    #[texture(1)]
    #[sampler(2)]
    pub base: Option<Handle<Image>>,
    pub alpha_mode: AlphaMode,
    pub no_depth_test: bool,
}

impl From<&SpriteMaterial> for SpriteKey {
    fn from(m: &SpriteMaterial) -> Self {
        Self { no_depth_test: m.no_depth_test }
    }
}

impl Material for SpriteMaterial {
    fn vertex_shader() -> ShaderRef {
        SPRITE_SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        SPRITE_SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
            if key.bind_group_data.no_depth_test {
                depth.depth_compare = Some(CompareFunction::Always);
            }
        }
        Ok(())
    }
}

/// The state of a sprite entity.
#[derive(Component, Clone, Debug, Serialize, Deserialize)]
pub struct SourceSprite {
    pub on: bool,
    pub mode: u8,
    /// Size of the visibility test area (`GlowProxySize`).
    pub proxy_size: f32,
    /// Smoothed share of the glow the view sees.
    #[serde(skip)]
    pub visible: f32,
    #[serde(skip)]
    pub material: Handle<SpriteMaterial>,
    #[serde(skip)]
    pub quad: Option<Entity>,
}

impl crate::save::Restore for SourceSprite {
    fn keep(&mut self, live: Self) {
        self.material = live.material;
        self.quad = live.quad;
    }
}

pub fn register_save(r: &mut crate::save::SaveRegistry) {
    r.component::<SourceSprite>("sprite");
}

pub struct SpritePlugin;

impl Plugin for SpritePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<SpriteMaterial>::default())
            .add_systems(PostUpdate, (update_sprites, glow_visibility).chain().after(TransformSystems::Propagate));
        let world = app.world_mut();
        for class in ["env_sprite", "env_glow", "env_sprite_oriented"] {
            world.resource_mut::<SpawnHandlers>().register(class, spawn_sprite);
            world.resource_mut::<InputHandlers>().register(class, sprite_input);
        }
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// `$spriteorientation` as the shader's number.
fn orientation(name: &str) -> f32 {
    match name.to_ascii_lowercase().as_str() {
        "parallel_upright" => 1.0,
        "oriented" | "vp_parallel_oriented" => 2.0,
        _ => 0.0,
    }
}

/// A sprite material file's texture, size and orientation; `.spr` names
/// stand for the material of the same name.
pub struct SpriteSource {
    pub base: Option<Handle<Image>>,
    pub size: Vec2,
    pub orientation: f32,
}

pub fn load_sprite_source(fs: &GameFs, textures: &mut TextureCache, images: &mut Assets<Image>, model: &str) -> SpriteSource {
    let name = model.replace('\\', "/").to_ascii_lowercase();
    let name = name.trim_start_matches("materials/").trim_end_matches(".vmt").trim_end_matches(".spr");
    let vmt = load_vmt(fs, &format!("materials/{name}.vmt"));
    let srgb = !vmt.as_ref().is_some_and(|v| v.get_bool("$nosrgb"));
    let tex = vmt.as_ref().and_then(|v| v.texture_path("$basetexture")).and_then(|p| textures.load(fs, images, &p, srgb));
    let orientation = vmt.as_ref().and_then(|v| v.get("$spriteorientation")).map(orientation).unwrap_or(0.0);
    SpriteSource {
        size: tex.as_ref().map(|t| Vec2::new(t.width as f32, t.height as f32)).unwrap_or(Vec2::splat(64.0)),
        base: tex.map(|t| t.handle),
        orientation,
    }
}

/// The quad sprites are drawn on (corners in y and z, -1 to 1).
pub fn sprite_quad() -> Mesh {
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0, -1.0, -1.0], [0.0, 1.0, -1.0], [0.0, 1.0, 1.0], [0.0, -1.0, 1.0]])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]])
        .with_inserted_indices(Indices::U16(vec![0, 1, 2, 0, 2, 3]))
}

fn parse_color(s: &str) -> Vec3 {
    let v: Vec<f32> = s.split_whitespace().filter_map(|p| p.parse().ok()).collect();
    match v.as_slice() {
        [r, g, b, ..] => Vec3::new(*r, *g, *b),
        _ => Vec3::splat(255.0),
    }
}

fn spawn_sprite(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let model = me.get("model").unwrap_or("sprites/glow01.vmt").to_owned();
    let source = {
        let fs = world.resource::<GameFs>().clone();
        world.resource_scope(|world, mut textures: Mut<TextureCache>| {
            let mut images = world.resource_mut::<Assets<Image>>();
            load_sprite_source(&fs, &mut textures, &mut images, &model)
        })
    };
    let mode = me.get_i32("rendermode").unwrap_or(0).clamp(0, 10) as u8;
    let scale = me.get_f32("scale").filter(|s| *s > 0.0).unwrap_or(1.0);
    let color = me.get("rendercolor").map(parse_color).unwrap_or(Vec3::splat(255.0)) / 255.0;
    let linear = Vec3::new(srgb_to_linear(color.x), srgb_to_linear(color.y), srgb_to_linear(color.z));
    let alpha = me.get_f32("renderamt").unwrap_or(255.0).clamp(0.0, 255.0) / 255.0;
    let hdr = me.get_f32("hdrcolorscale").unwrap_or(1.0);
    let additive = matches!(mode, MODE_GLOW | MODE_TRANS_ADD | MODE_WORLD_GLOW);
    let glow = matches!(mode, MODE_GLOW | MODE_WORLD_GLOW);
    let mut orientation = source.orientation;
    if me.classname == "env_sprite_oriented" {
        orientation = 2.0;
    }
    let material = world.resource_mut::<Assets<SpriteMaterial>>().add(SpriteMaterial {
        params: SpriteParams {
            color: (linear * hdr).extend(alpha),
            shape: Vec4::new(source.size.x * scale * 0.5, source.size.y * scale * 0.5, orientation, mode as f32),
            state: Vec4::new(if glow { 0.0 } else { 1.0 }, 1.0, 0.0, 0.0),
        },
        base: source.base,
        alpha_mode: if additive { AlphaMode::Add } else { AlphaMode::Blend },
        no_depth_test: glow,
    });
    let mesh = world.resource_mut::<Assets<Mesh>>().add(sprite_quad());
    // Unnamed sprites always show; named ones wait for ShowSprite unless they start on.
    let on = me.targetname.is_none() || me.has_flag(1);
    let quad = world
        .spawn((
            Name::new("sprite"),
            Mesh3d(mesh),
            MeshMaterial3d(material.clone()),
            Transform::IDENTITY,
            bevy::camera::visibility::NoFrustumCulling,
            if on { Visibility::Inherited } else { Visibility::Hidden },
        ))
        .id();
    world.entity_mut(e).add_child(quad);
    world.entity_mut(e).insert(SourceSprite {
        on,
        mode,
        proxy_size: me.get_f32("glowproxysize").unwrap_or(2.0).max(0.5),
        visible: 0.0,
        material,
        quad: Some(quad),
    });
}

fn sprite_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut s) = world.get_mut::<SourceSprite>(call.target) else { return false };
    let f = source_formats::kv::parse_f32_lenient(&call.param);
    match call.input.as_str() {
        "showsprite" | "turnon" => s.on = true,
        "hidesprite" | "turnoff" => s.on = false,
        "togglesprite" | "toggle" => s.on = !s.on,
        "alpha" | "color" | "setscale" | "colorredvalue" | "colorgreenvalue" | "colorbluevalue" => {
            let material = s.material.clone();
            let mut mats = world.resource_mut::<Assets<SpriteMaterial>>();
            if let Some(mut m) = mats.get_mut(&material) {
                match call.input.as_str() {
                    "alpha" => m.params.color.w = (f / 255.0).clamp(0.0, 1.0),
                    "color" => {
                        let c = parse_color(&call.param) / 255.0;
                        let l = Vec3::new(srgb_to_linear(c.x), srgb_to_linear(c.y), srgb_to_linear(c.z));
                        m.params.color = l.extend(m.params.color.w);
                    }
                    "setscale" => {
                        let half = m.params.shape.truncate().truncate();
                        let ratio = half.y / half.x.max(1e-3);
                        m.params.shape.x = f.max(0.01) * 32.0;
                        m.params.shape.y = f.max(0.01) * 32.0 * ratio;
                    }
                    _ => {}
                }
            }
        }
        _ => return false,
    }
    true
}

/// Shows and hides sprite quads with their entity's state.
fn update_sprites(sprites: Query<&SourceSprite, Changed<SourceSprite>>, mut vis: Query<&mut Visibility>) {
    for s in &sprites {
        if let Some(q) = s.quad
            && let Ok(mut v) = vis.get_mut(q)
        {
            let want = if s.on { Visibility::Inherited } else { Visibility::Hidden };
            if *v != want {
                *v = want;
            }
        }
    }
}

/// Glows: the share of five points around the origin (centre and the
/// corners of the proxy area, facing the view) that the view can see,
/// eased in and out over about a tenth of a second.
fn glow_visibility(
    time: Res<Time>,
    physics: Physics,
    main: Query<&GlobalTransform, With<MainCamera>>,
    mut sprites: Query<(&GlobalTransform, &mut SourceSprite)>,
    mut materials: ResMut<Assets<SpriteMaterial>>,
) {
    let Ok(eye) = main.single() else { return };
    let (eye_pos, right, up) = (eye.translation(), Vec3::from(eye.right()), Vec3::from(eye.up()));
    let rate = (time.delta_secs() * 10.0).min(1.0);
    for (gt, mut s) in &mut sprites {
        if !s.on || !matches!(s.mode, MODE_GLOW | MODE_WORLD_GLOW) {
            continue;
        }
        let centre = gt.translation();
        let r = s.proxy_size * 0.5;
        let points = [Vec3::ZERO, (right + up) * r, (right - up) * r, (-right + up) * r, (-right - up) * r];
        let seen = points
            .iter()
            .filter(|&&o| {
                let target = centre + o;
                let t = physics.trace(eye_pos, target, Vec3::ZERO, Vec3::ZERO, crate::collision::mask::SOLID_ALL, &[]);
                !t.hit() || t.fraction > 0.999
            })
            .count() as f32
            / points.len() as f32;
        let visible = s.visible + (seen - s.visible) * rate;
        if (visible - s.visible).abs() > 1e-3 || (seen == 0.0 && s.visible != 0.0) {
            s.visible = if seen == 0.0 && visible < 1e-3 { 0.0 } else { visible };
            if let Some(mut m) = materials.get_mut(&s.material) {
                m.params.state.x = s.visible;
            }
        }
    }
}
