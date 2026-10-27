//! VMT materials to render materials (`WorldMaterial` for brushes,
//! `ModelMaterial` for studio models).

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::render_resource::Face;
use source_formats::vmt::{Vmt, material_path};

use super::source_material::{ModelMaterial, WorldMaterial, model_flags, world_flags};
use super::textures::{TextureCache, TextureInfo};
use crate::vfs::GameFs;

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
}

/// Per-map caches of world and model materials.
#[derive(Resource, Default)]
pub struct MaterialCache {
    world: HashMap<String, WorldMaterialInfo>,
    model: HashMap<(String, bool), ModelMaterialInfo>,
    pub lightmap: Option<Handle<Image>>,
    /// Fallback cube map for `env_cubemap` (nearest probe is chosen per model later).
    pub default_cubemap: Option<Handle<Image>>,
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

impl MaterialCache {
    pub fn clear(&mut self) {
        self.world.clear();
        self.model.clear();
        self.lightmap = None;
        self.default_cubemap = None;
    }

    pub fn world(
        &mut self,
        name: &str,
        ctx: &mut MaterialContext,
        materials: &mut Assets<WorldMaterial>,
    ) -> WorldMaterialInfo {
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
                }
                if let Some(env) = ctx.cubemap(vmt, "$envmap").or_else(|| {
                    vmt.get("$envmap").is_some_and(|e| e.eq_ignore_ascii_case("env_cubemap")).then(|| self.default_cubemap.clone()).flatten()
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
                    Blend::Translucent | Blend::Additive => p.flags |= world_flags::TRANSLUCENT,
                    Blend::Opaque => {}
                }
                if surface.unlit {
                    p.flags |= world_flags::UNLIT;
                }
                if vmt.shader == "water" {
                    // Stand-in until the water shader exists: murky fog colour, dimmed
                    // to roughly the brightness of the surrounding lightmaps.
                    let fog = vmt.get_vec3("$fogcolor").unwrap_or(Vec3::new(0.2, 0.25, 0.2));
                    p.color = (fog * 0.12).extend(0.95);
                    p.flags |= world_flags::TRANSLUCENT | world_flags::UNLIT;
                    m.base = None;
                    m.alpha_mode = AlphaMode::Blend;
                }
                if vmt.get_bool("$nocull") {
                    m.cull = None;
                }
                if vmt.shader != "water" {
                    m.alpha_mode = alpha_mode(surface.blend, vmt);
                }
                if vmt.get_bool("$decal") {
                    m.depth_bias = 2;
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
                if let Some(env) = ctx.cubemap(vmt, "$envmap").or_else(|| {
                    vmt.get("$envmap").is_some_and(|e| e.eq_ignore_ascii_case("env_cubemap")).then(|| self.default_cubemap.clone()).flatten()
                }) {
                    m.envmap = Some(env);
                    p.flags |= model_flags::ENVMAP;
                    p.envmap_tint = vmt.get_vec3("$envmaptint").unwrap_or(Vec3::ONE).extend(1.0);
                    if let Some(t) = ctx.tex(vmt, "$envmapmask", false) {
                        m.envmask = Some(t.handle);
                        p.flags |= model_flags::ENVMAPMASK;
                    }
                    if vmt.get_bool("$basealphaenvmapmask") {
                        p.flags |= model_flags::BASEALPHA_ENVMAPMASK;
                    }
                    if vmt.get_bool("$normalmapalphaenvmapmask") && m.bump.is_some() {
                        p.flags |= model_flags::NORMALALPHA_ENVMAPMASK;
                    }
                }
                if vmt.get_bool("$phong") {
                    p.flags |= model_flags::PHONG;
                    let fresnel = vmt.get_vec3("$phongfresnelranges").unwrap_or(Vec3::new(0.0, 0.5, 1.0));
                    p.phong = Vec4::new(
                        vmt.get_f32("$phongexponent").unwrap_or(5.0),
                        vmt.get_f32("$phongboost").unwrap_or(1.0),
                        fresnel.x,
                        fresnel.z,
                    );
                    if vmt.get_bool("$basemapalphaphongmask") {
                        p.flags |= model_flags::BASEMAPALPHA_PHONGMASK;
                    }
                }
                if vmt.get_bool("$selfillum") {
                    p.flags |= model_flags::SELFILLUM;
                    p.selfillum_tint = vmt.get_vec3("$selfillumtint").unwrap_or(Vec3::ONE).extend(1.0);
                }
                let color = vmt.get_vec3("$color2").or_else(|| vmt.get_vec3("$color")).unwrap_or(Vec3::ONE);
                p.color = color.extend(vmt.get_f32("$alpha").unwrap_or(1.0));
                match surface.blend {
                    Blend::AlphaTest => {
                        p.flags |= model_flags::ALPHATEST;
                        p.alpha_ref = vmt.get_f32("$alphatestreference").filter(|v| *v > 0.0).unwrap_or(0.5);
                    }
                    Blend::Translucent | Blend::Additive => p.flags |= model_flags::TRANSLUCENT,
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
        let info = ModelMaterialInfo { surface, handle: materials.add(m) };
        self.model.insert(key, info.clone());
        info
    }
}
