//! VMT materials to render materials (`WorldMaterial` for brushes,
//! `ModelMaterial` for studio models, `RefractMaterial` for models using the
//! `Refract` shader).

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::render_resource::Face;
use source_formats::vmt::{Vmt, material_path};
use source_formats::vtf;

use super::source_material::{DECAL_DEPTH_BIAS, ModelMaterial, RefractMaterial, WorldMaterial, model_flags, refract_flags, world_flags};
use super::textures::{TextureCache, TextureInfo};
use portal2_core::vfs::GameFs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    Opaque,
    AlphaTest,
    Translucent,
    Additive,
}

/// Material properties gameplay code cares about (portalability, sounds...).
#[derive(Clone, Debug)]
pub struct SurfaceInfo {
    pub name: String,
    pub vmt: Option<Arc<Vmt>>,
    pub shader: String,
    pub blend: Blend,
    pub no_portal: bool,
    pub surface_prop: String,
    pub unlit: bool,
}

#[derive(Clone, Debug)]
pub struct WorldMaterialInfo {
    pub surface: SurfaceInfo,
    pub handle: Handle<WorldMaterial>,
}

#[derive(Clone, Debug)]
pub struct ModelMaterialInfo {
    pub surface: SurfaceInfo,
    pub handle: Handle<ModelMaterial>,
    /// `$envmap env_cubemap`: the nearest cube map is filled in per instance.
    pub env_cubemap: bool,
}

#[derive(Clone, Debug)]
pub struct RefractMaterialInfo {
    pub handle: Handle<RefractMaterial>,
    /// `$envmap env_cubemap`: the nearest cube map is filled in per instance.
    pub env_cubemap: bool,
}

/// Per-map caches of world and model materials.
#[derive(Resource, Default)]
pub struct MaterialCache {
    world: HashMap<String, WorldMaterialInfo>,
    model: HashMap<(String, bool), ModelMaterialInfo>,
    /// `None` for materials whose shader is not `Refract`.
    refract: HashMap<String, Option<RefractMaterialInfo>>,
    /// Copies of `env_cubemap` materials with a cube map filled in, shared by
    /// every instance near the same probe.
    model_cubemaps: HashMap<(AssetId<ModelMaterial>, AssetId<Image>), Handle<ModelMaterial>>,
    refract_cubemaps: HashMap<(AssetId<RefractMaterial>, AssetId<Image>), Handle<RefractMaterial>>,
    pub lightmap: Option<Handle<Image>>,
    /// Fallback cube map for `env_cubemap` (nearest probe is chosen per model later).
    pub default_cubemap: Option<Handle<Image>>,
    /// The paint map and paint textures, on maps with paint.
    pub paint: Option<PaintTextures>,
}

/// What paintable world materials bind to draw paint (see docs/re/paint.md).
#[derive(Clone, Debug)]
pub struct PaintTextures {
    pub map: Handle<Image>,
    pub splat: Option<Handle<Image>>,
    pub bubble_layout: Option<Handle<Image>>,
    pub bubble: Option<Handle<Image>>,
    pub envmap: Option<Handle<Image>>,
}

pub struct MaterialContext<'a> {
    pub fs: &'a GameFs,
    pub textures: &'a mut TextureCache,
    pub images: &'a mut Assets<Image>,
}

pub fn load_vmt(fs: &GameFs, path: &str) -> Option<Vmt> {
    let text = fs.read_string(path)?;
    let mut include = |p: &str| fs.read_string(p);
    Vmt::parse(&text, &mut include)
}

fn blend_of(vmt: &Vmt) -> Blend {
    if vmt.get_bool("$additive") {
        Blend::Additive
    } else if vmt.get_bool("$translucent") || vmt.get_f32("$alpha").is_some_and(|a| a < 1.0) {
        Blend::Translucent
    } else if vmt.get_bool("$alphatest") {
        Blend::AlphaTest
    } else {
        Blend::Opaque
    }
}

fn alpha_mode(blend: Blend, vmt: &Vmt) -> AlphaMode {
    match blend {
        Blend::AlphaTest if vmt.get_bool("$allowalphatocoverage") => AlphaMode::AlphaToCoverage,
        Blend::Opaque | Blend::AlphaTest => AlphaMode::Opaque,
        Blend::Translucent => AlphaMode::Blend,
        Blend::Additive => {
            let _ = vmt;
            AlphaMode::Add
        }
    }
}

fn surface_info(path: &str, vmt: Option<&Vmt>) -> SurfaceInfo {
    match vmt {
        Some(vmt) => SurfaceInfo {
            name: path.to_owned(),
            shader: vmt.shader.clone(),
            blend: blend_of(vmt),
            no_portal: vmt.get_bool("%noportal"),
            surface_prop: vmt.get("$surfaceprop").unwrap_or("default").to_ascii_lowercase(),
            unlit: matches!(vmt.shader.as_str(), "unlitgeneric" | "unlittwotexture" | "solidenergy" | "sprite" | "spritecard"),
            vmt: Some(Arc::new(vmt.clone())),
        },
        None => SurfaceInfo {
            name: path.to_owned(),
            vmt: None,
            shader: String::new(),
            blend: Blend::Opaque,
            no_portal: false,
            surface_prop: "default".into(),
            unlit: false,
        },
    }
}

impl MaterialCache {
    /// The world material a handle belongs to, by name.
    pub fn world_by_handle(&self, handle: &Handle<WorldMaterial>) -> Option<&WorldMaterialInfo> {
        self.world.values().find(|i| i.handle == *handle)
    }
}

impl MaterialContext<'_> {
    fn tex(&mut self, vmt: &Vmt, param: &str, srgb: bool) -> Option<TextureInfo> {
        let path = vmt.texture_path(param)?;
        if path.ends_with("/env_cubemap.vtf") {
            return None;
        }
        self.textures.load(self.fs, self.images, &path, srgb)
    }

    /// Cube maps prefer the `.hdr.vtf` variant when rendering in HDR.
    fn cubemap(&mut self, vmt: &Vmt, param: &str) -> Option<Handle<Image>> {
        let path = vmt.texture_path(param)?;
        if path.ends_with("/env_cubemap.vtf") {
            return None;
        }
        let hdr = format!("{}.hdr.vtf", path.trim_end_matches(".vtf"));
        self.textures
            .load(self.fs, self.images, &hdr, false)
            .or_else(|| self.textures.load(self.fs, self.images, &path, true))
            .map(|t| t.handle)
    }

    fn missing(&mut self) -> Handle<Image> {
        self.textures.missing(self.images)
    }
}

/// `SolidEnergy` flow materials (the emancipation field): the ripples in the
/// base texture's alpha scroll along `$flowmap` in two phases half
/// a cycle apart (`$flow_timeintervalinseconds`, `$flow_uvscrolldistance`),
/// tinted `$flow_color` × `$outputintensity`, masked by `$flowbounds` and
/// added to the scene. How the shader combines its textures was not
/// traced; the alpha (a dim copy of the green ripples) matches the game's
/// brightness at the same exposure.
fn solid_energy_material(vmt: &Vmt, m: &mut WorldMaterial, ctx: &mut MaterialContext) {
    let p = &mut m.params;
    let tint = vmt.get_vec3("$flow_color").unwrap_or(Vec3::new(0.025, 0.08, 0.1));
    let intensity = vmt.get_f32("$outputintensity").unwrap_or(1.0);
    p.color = (tint * intensity).extend(1.0);
    p.flags &= !(world_flags::LIGHTMAPPED | world_flags::BUMP | world_flags::SSBUMP | world_flags::ENVMAP);
    p.flags |= world_flags::SOLID_ENERGY | world_flags::UNLIT | world_flags::TRANSLUCENT | world_flags::ADDITIVE;
    m.base = ctx.tex(vmt, "$basetexture", false).map(|t| t.handle);
    if let Some(flow) = ctx.tex(vmt, "$flowmap", false) {
        m.base2 = Some(flow.handle);
        p.flags |= world_flags::BASE2;
    }
    m.detail = ctx.tex(vmt, "$flowbounds", false).map(|t| t.handle);
    if m.detail.is_some() {
        p.flags |= world_flags::DETAIL;
    }
    p.detail = Vec4::new(
        vmt.get_f32("$flow_timeintervalinseconds").filter(|v| *v > 0.0).unwrap_or(1.2),
        vmt.get_f32("$flow_uvscrolldistance").unwrap_or(0.25),
        vmt.get_f32("$flow_worlduvscale").filter(|v| *v > 0.0).unwrap_or(0.008),
        0.0,
    );
    m.alpha_mode = AlphaMode::Add;
}

/// The parts of Portal 2's `Water` shader we draw: the water's fog colour,
/// lit by the lightmap (`$lightmapwaterfog`) since the bottom is out of
/// sight a fog length below the surface; for goo the `$basetexture` on top,
/// its alpha marking the floating scum, scrolled along the `$flowmap` in two
/// phases half a cycle apart (`$color_flow_*`); and the reflection as the
/// map's cube map times `$reflectamount` and `$reflecttint`.
fn water_material(vmt: &Vmt, m: &mut WorldMaterial, ctx: &mut MaterialContext, cubemap: Option<Handle<Image>>) {
    let p = &mut m.params;
    let color255 = |key: &str, default: Vec3| {
        let raw = vmt.get(key).unwrap_or("");
        let v = vmt.get_vec3(key).unwrap_or(default);
        if raw.trim_start().starts_with('[') { v } else { v / 255.0 }
    };
    let fog = color255("$fogcolor", Vec3::new(0.2, 0.25, 0.2));
    let fog = Color::srgb(fog.x, fog.y, fog.z).to_linear();
    p.color = Vec4::new(fog.red, fog.green, fog.blue, 1.0);
    p.flags &= !(world_flags::TRANSLUCENT | world_flags::UNLIT | world_flags::BUMP | world_flags::SSBUMP);
    p.flags |= world_flags::WATER;
    if !vmt.get_bool("$lightmapwaterfog") {
        p.flags &= !world_flags::LIGHTMAPPED;
    }
    m.base = None;
    m.bump = None;
    if let Some(t) = ctx.tex(vmt, "$basetexture", true) {
        m.base = Some(t.handle);
        p.flags |= world_flags::WATER_BASE;
        if let Some(flow) = ctx.tex(vmt, "$flowmap", false) {
            m.base2 = Some(flow.handle);
            p.flags |= world_flags::BASE2;
        }
        p.detail = Vec4::new(
            vmt.get_f32("$color_flow_uvscale").filter(|v| *v > 0.0).unwrap_or(400.0),
            vmt.get_f32("$color_flow_timeintervalinseconds").filter(|v| *v > 0.0).unwrap_or(6.0),
            vmt.get_f32("$color_flow_uvscrolldistance").unwrap_or(0.1),
            0.0,
        );
    }
    p.flags &= !world_flags::VERTEX_BLEND;
    p.flags &= !world_flags::ENVMAP;
    if let Some(env) = cubemap
        && vmt.get("$reflecttexture").is_some()
    {
        let amount = vmt.get_f32("$reflectamount").unwrap_or(0.0);
        let tint = color255("$reflecttint", Vec3::ONE);
        let tint = Color::srgb(tint.x, tint.y, tint.z).to_linear();
        m.envmap = Some(env);
        p.flags |= world_flags::ENVMAP;
        p.envmap_tint = Vec4::new(tint.red * amount, tint.green * amount, tint.blue * amount, 1.0);
    }
    m.alpha_mode = AlphaMode::Opaque;
}

impl MaterialCache {
    pub fn clear(&mut self) {
        self.world.clear();
        self.model.clear();
        self.refract.clear();
        self.model_cubemaps.clear();
        self.refract_cubemaps.clear();
        self.lightmap = None;
        self.default_cubemap = None;
        self.paint = None;
    }

    pub fn world(&mut self, name: &str, ctx: &mut MaterialContext, materials: &mut Assets<WorldMaterial>) -> WorldMaterialInfo {
        let key = material_path(name);
        if let Some(info) = self.world.get(&key) {
            return info.clone();
        }
        let vmt = load_vmt(ctx.fs, &key);
        if vmt.is_none() {
            warn!("missing material {key}");
        }
        let surface = surface_info(&key, vmt.as_ref());
        let mut m = WorldMaterial { lightmap: self.lightmap.clone(), ..default() };
        let p = &mut m.params;
        p.flags |= world_flags::LIGHTMAPPED;
        match &vmt {
            None => m.base = Some(ctx.missing()),
            Some(vmt) => {
                m.base = ctx.tex(vmt, "$basetexture", true).map(|t| t.handle).or_else(|| Some(ctx.missing()));
                if let Some(t) = ctx.tex(vmt, "$basetexture2", true) {
                    m.base2 = Some(t.handle);
                    p.flags |= world_flags::BASE2 | world_flags::VERTEX_BLEND;
                }
                let ssbump = vmt.get_bool("$ssbump");
                if let Some(t) = ctx.tex(vmt, "$bumpmap", false).or_else(|| ctx.tex(vmt, "$normalmap", false)) {
                    m.bump = Some(t.handle);
                    p.flags |= world_flags::BUMP;
                    if ssbump {
                        p.flags |= world_flags::SSBUMP;
                    }
                    if p.flags & world_flags::VERTEX_BLEND != 0
                        && let Some(t) = ctx.tex(vmt, "$bumpmap2", false)
                    {
                        m.bump2 = Some(t.handle);
                        p.flags |= world_flags::BUMP2;
                    }
                }
                if p.flags & world_flags::VERTEX_BLEND != 0
                    && let Some(t) = ctx.tex(vmt, "$blendmodulatetexture", false)
                {
                    m.blend_modulate = Some(t.handle);
                    p.flags |= world_flags::BLEND_MODULATE;
                }
                if let Some(env) = ctx.cubemap(vmt, "$envmap").or_else(|| {
                    vmt.get("$envmap")
                        .is_some_and(|e| e.eq_ignore_ascii_case("env_cubemap"))
                        .then(|| self.default_cubemap.clone())
                        .flatten()
                }) {
                    m.envmap = Some(env);
                    p.flags |= world_flags::ENVMAP;
                    p.envmap_tint = vmt.get_vec3("$envmaptint").unwrap_or(Vec3::ONE).extend(1.0);
                    p.envmap_contrast = vmt.get_f32("$envmapcontrast").unwrap_or(0.0);
                    p.envmap_saturation = vmt.get_f32("$envmapsaturation").unwrap_or(1.0);
                    p.envmap_light_scale = vmt.get_f32("$envmaplightscale").unwrap_or(0.0);
                    if let Some(t) = ctx.tex(vmt, "$envmapmask", false) {
                        m.envmask = Some(t.handle);
                        p.flags |= world_flags::ENVMAPMASK;
                    }
                    if vmt.get_bool("$basealphaenvmapmask") {
                        p.flags |= world_flags::BASEALPHA_ENVMAPMASK;
                    }
                    if vmt.get_bool("$normalmapalphaenvmapmask") && m.bump.is_some() {
                        p.flags |= world_flags::NORMALALPHA_ENVMAPMASK;
                    }
                }
                if let Some(t) = ctx.tex(vmt, "$detail", true) {
                    m.detail = Some(t.handle);
                    p.flags |= world_flags::DETAIL;
                    p.detail = Vec4::new(
                        vmt.get_f32("$detailscale").unwrap_or(4.0),
                        vmt.get_f32("$detailblendfactor").unwrap_or(1.0),
                        vmt.get_f32("$detailblendmode").unwrap_or(0.0),
                        0.0,
                    );
                }
                if vmt.get_bool("$selfillum") {
                    p.flags |= world_flags::SELFILLUM;
                    p.selfillum_tint = vmt.get_vec3("$selfillumtint").unwrap_or(Vec3::ONE).extend(1.0);
                }
                let color = vmt.get_vec3("$color").unwrap_or(Vec3::ONE);
                let alpha = vmt.get_f32("$alpha").unwrap_or(1.0);
                p.color = color.extend(alpha);
                match surface.blend {
                    Blend::AlphaTest => {
                        p.flags |= world_flags::ALPHATEST;
                        p.alpha_ref = vmt.get_f32("$alphatestreference").filter(|v| *v > 0.0).unwrap_or(0.5);
                    }
                    Blend::Translucent => p.flags |= world_flags::TRANSLUCENT,
                    Blend::Additive => p.flags |= world_flags::TRANSLUCENT | world_flags::ADDITIVE,
                    Blend::Opaque => {}
                }
                if surface.unlit {
                    p.flags |= world_flags::UNLIT;
                }
                if vmt.shader == "water" {
                    water_material(vmt, &mut m, ctx, self.default_cubemap.clone());
                }
                let solid_energy = vmt.shader == "solidenergy" && vmt.get("$flowmap").is_some();
                if vmt.get_bool("$nocull") {
                    m.cull = None;
                }
                if vmt.shader != "water" {
                    m.alpha_mode = alpha_mode(surface.blend, vmt);
                }
                if solid_energy {
                    solid_energy_material(vmt, &mut m, ctx);
                }
                if vmt.get_bool("$decal") {
                    m.depth_bias = DECAL_DEPTH_BIAS;
                }
                // Translucent surfaces never take paint, nor do decals,
                // water or energy fields.
                let unpaintable = world_flags::TRANSLUCENT | world_flags::WATER | world_flags::SOLID_ENERGY | world_flags::UNLIT;
                if let Some(paint) = &self.paint
                    && m.params.flags & unpaintable == 0
                    && m.depth_bias == 0
                {
                    m.params.flags |= world_flags::PAINT;
                    m.paint = Some(paint.map.clone());
                    m.paint_splat = paint.splat.clone();
                    m.paint_bubble_layout = paint.bubble_layout.clone();
                    m.paint_bubble = paint.bubble.clone();
                    m.paint_envmap = paint.envmap.clone();
                }
            }
        }
        let info = WorldMaterialInfo { surface, handle: materials.add(m) };
        self.world.insert(key, info.clone());
        info
    }

    /// Model material; `baked` selects per-vertex static prop lighting.
    pub fn model(
        &mut self,
        path: &str,
        baked: bool,
        ctx: &mut MaterialContext,
        materials: &mut Assets<ModelMaterial>,
    ) -> ModelMaterialInfo {
        let key = (path.to_owned(), baked);
        if let Some(info) = self.model.get(&key) {
            return info.clone();
        }
        let vmt = load_vmt(ctx.fs, path);
        let surface = surface_info(path, vmt.as_ref());
        let mut m = ModelMaterial::default();
        let p = &mut m.params;
        if baked {
            p.flags |= model_flags::BAKED;
        }
        match &vmt {
            None => m.base = Some(ctx.missing()),
            Some(vmt) => {
                m.base = ctx.tex(vmt, "$basetexture", true).map(|t| t.handle).or_else(|| Some(ctx.missing()));
                if let Some(t) = ctx.tex(vmt, "$bumpmap", false) {
                    m.bump = Some(t.handle);
                    p.flags |= model_flags::BUMP;
                }
                // Reflection settings are kept even when the cube map itself only
                // arrives per instance (`env_cubemap` resolved near the model).
                if vmt.get("$envmap").is_some() {
                    p.envmap_tint = vmt.get_vec3("$envmaptint").unwrap_or(Vec3::ONE).extend(1.0);
                    // The bumped shader variant takes the first component only.
                    let contrast = vmt.get_vec3("$envmapcontrast").unwrap_or(Vec3::ZERO);
                    p.envmap_contrast = if m.bump.is_some() { Vec3::splat(contrast.x) } else { contrast }.extend(0.0);
                    p.envmap_saturation = vmt.get_vec3("$envmapsaturation").unwrap_or(Vec3::ONE).extend(1.0);
                    if let Some(t) = ctx.tex(vmt, "$envmapmask", false) {
                        m.envmask = Some(t.handle);
                        p.flags |= model_flags::ENVMAPMASK;
                    }
                    if vmt.get_bool("$basealphaenvmapmask") {
                        p.flags |= model_flags::BASEALPHA_ENVMAPMASK;
                    }
                    // Phong materials honour the flag with or without a normal map.
                    if vmt.get_bool("$normalmapalphaenvmapmask") && (m.bump.is_some() || vmt.get_bool("$phong")) {
                        p.flags |= model_flags::NORMALALPHA_ENVMAPMASK;
                    }
                    if vmt.get_bool("$invertphongmask") {
                        p.flags |= model_flags::INVERT_PHONG_ENVMAPMASK;
                    }
                }
                if let Some(env) = ctx.cubemap(vmt, "$envmap").or_else(|| {
                    vmt.get("$envmap")
                        .is_some_and(|e| e.eq_ignore_ascii_case("env_cubemap"))
                        .then(|| self.default_cubemap.clone())
                        .flatten()
                }) {
                    m.envmap = Some(env);
                    p.flags |= model_flags::ENVMAP;
                }
                if vmt.get_bool("$phong") {
                    p.flags |= model_flags::PHONG;
                    // A constant exponent overrides the exponent texture's red channel.
                    let exponent_texture = ctx.tex(vmt, "$phongexponenttexture", false);
                    let exponent = match (vmt.get_f32("$phongexponent"), &exponent_texture) {
                        (Some(e), _) => e,
                        (None, Some(_)) => 0.0,
                        (None, None) => 5.0,
                    };
                    if let Some(t) = exponent_texture {
                        m.exponent = Some(t.handle);
                        p.flags |= model_flags::EXPONENT_TEXTURE;
                    }
                    p.phong = Vec4::new(exponent, vmt.get_f32("$phongboost").unwrap_or(1.0), 0.0, 0.0);
                    p.fresnel = vmt
                        .get_vec3("$phongfresnelranges")
                        .unwrap_or(Vec3::new(0.0, 0.5, 1.0))
                        .extend(vmt.get_f32("$envmapfresnel").unwrap_or(0.0));
                    if vmt.get_bool("$phongalbedotint") {
                        p.flags |= model_flags::ALBEDO_TINT;
                    } else if let Some(tint) = vmt.get_vec3("$phongtint") {
                        p.phong_tint = tint.extend(1.0);
                    }
                    if vmt.get_bool("$basemapalphaphongmask") {
                        p.flags |= model_flags::BASEMAPALPHA_PHONGMASK;
                    }
                    if vmt.get_bool("$basemapluminancephongmask") {
                        p.flags |= model_flags::BASEMAPLUMINANCE_PHONGMASK;
                    }
                    // Phong materials light their diffuse with half-Lambert unless told not to.
                    if !vmt.get_bool("$phongdisablehalflambert") {
                        p.flags |= model_flags::HALF_LAMBERT;
                    }
                    if let Some(t) = ctx.tex(vmt, "$lightwarptexture", true) {
                        m.lightwarp = Some(t.handle);
                        p.flags |= model_flags::LIGHTWARP;
                    }
                } else if vmt.get_bool("$halflambert") {
                    p.flags |= model_flags::HALF_LAMBERT;
                }
                if vmt.get_bool("$selfillum") {
                    p.flags |= model_flags::SELFILLUM;
                    p.selfillum_tint = vmt.get_vec3("$selfillumtint").unwrap_or(Vec3::ONE).extend(1.0);
                    if let Some(t) = ctx.tex(vmt, "$selfillummask", false) {
                        m.selfillum_mask = Some(t.handle);
                        p.flags |= model_flags::SELFILLUM_MASK;
                    }
                }
                let color = vmt.get_vec3("$color2").or_else(|| vmt.get_vec3("$color")).unwrap_or(Vec3::ONE);
                p.color = color.extend(vmt.get_f32("$alpha").unwrap_or(1.0));
                match surface.blend {
                    Blend::AlphaTest => {
                        p.flags |= model_flags::ALPHATEST;
                        p.alpha_ref = vmt.get_f32("$alphatestreference").filter(|v| *v > 0.0).unwrap_or(0.5);
                    }
                    Blend::Translucent => p.flags |= model_flags::TRANSLUCENT,
                    Blend::Additive => p.flags |= model_flags::TRANSLUCENT | model_flags::ADDITIVE,
                    Blend::Opaque => {}
                }
                if surface.unlit {
                    p.flags |= model_flags::UNLIT;
                }
                if vmt.get_bool("$vertexcolor") {
                    p.flags |= model_flags::VERTEX_COLOR;
                }
                if vmt.get_bool("$nocull") {
                    m.cull = None;
                }
                m.alpha_mode = alpha_mode(surface.blend, vmt);
            }
        }
        let _ = Face::Back;
        let env_cubemap = vmt.as_ref().and_then(|v| v.get("$envmap")).is_some_and(|e| e.eq_ignore_ascii_case("env_cubemap"));
        let info = ModelMaterialInfo { surface, handle: materials.add(m), env_cubemap };
        self.model.insert(key, info.clone());
        info
    }

    /// The material of a model surface if its shader is `Refract`.
    pub fn refract(
        &mut self,
        path: &str,
        ctx: &mut MaterialContext,
        materials: &mut Assets<RefractMaterial>,
    ) -> Option<RefractMaterialInfo> {
        if let Some(info) = self.refract.get(path) {
            return info.clone();
        }
        let info = load_vmt(ctx.fs, path).filter(|vmt| vmt.shader == "refract").map(|vmt| {
            let mut m = RefractMaterial::default();
            let p = &mut m.params;
            let mut translucent_normal = false;
            if let Some(t) = ctx.tex(&vmt, "$normalmap", false) {
                translucent_normal = t.flags & (vtf::flags::ONEBITALPHA | vtf::flags::EIGHTBITALPHA) != 0;
                m.normal = Some(t.handle);
                p.flags |= refract_flags::NORMALMAP;
            }
            // Local refraction looks into the base texture; without one the
            // game would fall back to the scene, so do the same.
            let local = vmt.get_bool("$localrefract");
            if let Some(t) = local.then(|| ctx.tex(&vmt, "$basetexture", true)).flatten() {
                // A whole-number ratio, as the game computes it.
                p.refract.z = (t.height / t.width.max(1)) as f32;
                m.base = Some(t.handle);
                p.flags |= refract_flags::LOCAL;
            }
            p.refract.x = vmt.get_f32("$refractamount").unwrap_or(2.0);
            p.refract.y = vmt.get_f32("$localrefractdepth").unwrap_or(0.05);
            p.refract_tint = gamma_to_linear(vmt.get_vec3("$refracttint").unwrap_or(Vec3::ONE)).extend(1.0);
            if vmt.get_f32("$bluramount").unwrap_or(0.0) >= 1.0 {
                p.flags |= refract_flags::BLUR;
            }
            if let Some(t) = ctx.tex(&vmt, "$refracttinttexture", true) {
                m.tint = Some(t.handle);
                p.flags |= refract_flags::TINT_TEXTURE;
            }
            let has_envmap = vmt.get("$envmap").is_some();
            let env_cubemap = vmt.get("$envmap").is_some_and(|e| e.eq_ignore_ascii_case("env_cubemap"));
            if has_envmap {
                p.envmap_tint = gamma_to_linear(vmt.get_vec3("$envmaptint").unwrap_or(Vec3::ONE)).extend(1.0);
                p.envmap_contrast = vmt.get_vec3("$envmapcontrast").unwrap_or(Vec3::ZERO).extend(0.0);
                p.envmap_saturation = vmt.get_vec3("$envmapsaturation").unwrap_or(Vec3::ONE).extend(1.0);
                if let Some(env) = ctx.cubemap(&vmt, "$envmap").or_else(|| env_cubemap.then(|| self.default_cubemap.clone()).flatten()) {
                    m.envmap = Some(env);
                    p.flags |= refract_flags::ENVMAP;
                }
            }
            if vmt.get_bool("$nocull") {
                m.cull = None;
            }
            // With a cube map the game draws the result as is (the scene behind
            // is already in it); otherwise it blends like its other shaders, by
            // the normal map's alpha.
            m.alpha_mode = if has_envmap {
                AlphaMode::Opaque
            } else if vmt.get_bool("$additive") {
                m.params.flags |= refract_flags::ADDITIVE;
                AlphaMode::Add
            } else if translucent_normal || vmt.get_bool("$vertexalpha") || vmt.get_f32("$alpha").is_some_and(|a| a < 1.0) {
                AlphaMode::Blend
            } else {
                AlphaMode::Opaque
            };
            RefractMaterialInfo { handle: materials.add(m), env_cubemap }
        });
        self.refract.insert(path.to_owned(), info.clone());
        info
    }

    /// `base` with its `env_cubemap` resolved to `env`, one copy per pair.
    pub fn model_with_cubemap(
        &mut self,
        base: &Handle<ModelMaterial>,
        env: &Handle<Image>,
        materials: &mut Assets<ModelMaterial>,
    ) -> Handle<ModelMaterial> {
        self.model_cubemaps
            .entry((base.id(), env.id()))
            .or_insert_with(|| {
                let mut m = materials.get(base).cloned().unwrap_or_default();
                m.envmap = Some(env.clone());
                m.params.flags |= model_flags::ENVMAP;
                materials.add(m)
            })
            .clone()
    }

    /// `base` with its `env_cubemap` resolved to `env`, one copy per pair.
    pub fn refract_with_cubemap(
        &mut self,
        base: &Handle<RefractMaterial>,
        env: &Handle<Image>,
        materials: &mut Assets<RefractMaterial>,
    ) -> Handle<RefractMaterial> {
        self.refract_cubemaps
            .entry((base.id(), env.id()))
            .or_insert_with(|| {
                let mut m = materials.get(base).cloned().unwrap_or_default();
                m.envmap = Some(env.clone());
                m.params.flags |= refract_flags::ENVMAP;
                materials.add(m)
            })
            .clone()
    }
}

/// Colour parameters the game passes through its gamma-to-linear table:
/// channels up to 1 are raised to 2.2 from the nearest 0-255 step, except that
/// 0.95 and above count as exactly 1; larger values pass unchanged.
fn gamma_to_linear(c: Vec3) -> Vec3 {
    let f = |v: f32| {
        if v > 1.0 {
            v
        } else if v >= 0.95 {
            1.0
        } else if v <= 0.0 {
            0.0
        } else {
            ((v * 255.0).round() / 255.0).powf(2.2)
        }
    };
    Vec3::new(f(c.x), f(c.y), f(c.z))
}
