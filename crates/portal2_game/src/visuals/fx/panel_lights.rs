//! Glow from the indicator lamps on GLaDOS's panel machinery: the arms that
//! carry the moving wall panels, the telescoping arms and the shields
//! (`models/anim_wp/...`) have small self-lit lamps and strips, cyan when
//! they work and red on an arm's error skin.
//!
//! Each model is looked at once per skin: the self-lit part of a material
//! is its base texture's alpha (or `$selfillummask`), so the lit blobs in
//! that mask are found, and each blob's middle is found on the mesh through
//! the triangle whose texture coordinates cover it. That gives every lamp a
//! place and facing on the bone that carries it, a colour (the lit texels'
//! mean, times `$selfillumtint`) and a size (the blob's texels over the
//! triangle's texture-to-world scale). A long thin blob becomes a strip.
//!
//! Each frame every lamp near the view becomes a small, soft light just off
//! its surface, as bright as its size, lighting the arm around it and the
//! surfaces close by. A lamp the view cannot see past the walls and the
//! panels (an arm folded away behind its panel) fades out, so no light
//! seeps through a closed wall; lamps close together share one light.
//!
//! `r_fx_panellights` scales them all (0 switches them off). They need the
//! FX lights feature, so the faithful renderer is unchanged.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use source_formats::studio::StudioModel;
use source_formats::vtf::Vtf;

use super::lights::{FxLightData, FxLightShape, FxLights, FxProduceSet, light_flags};
use super::{FxSettings, features};
use crate::devices::laser::srgb_to_linear;
use portal2_core::entity::MapEntity;
use portal2_core::vfs::GameFs;
use portal2_core::view::MainCamera;
use portal2_render::models::StudioAnimator;

/// The panel machinery's models.
const MODEL_PREFIX: &str = "models/anim_wp/";
/// Masks are read at no more than this size.
const MAX_MASK_SIZE: u32 = 512;
/// Mask texels at least this strong belong to a lamp.
const MASK_THRESHOLD: f32 = 0.3;
/// A blob weaker than this many full texels (at the size read) is noise.
const MIN_BLOB_TEXELS: f32 = 1.0;
/// A blob this many times longer than wide is a strip.
const STRIP_ASPECT: f32 = 4.0;
/// A mask lit over more than this share of its texels is not a few lamps
/// (a whole self-lit surface), and gives no light.
const MAX_LIT_SHARE: f32 = 0.2;
/// Lamps on one bone closer than this are one light.
const MERGE_ON_BONE: f32 = 6.0;

/// Light per square unit of lamp, on the lightmaps' scale, and its bounds:
/// a soft wash close by, never a flood.
const INTENSITY_PER_AREA: f32 = 1.0;
const MIN_INTENSITY: f32 = 2.0;
const MAX_INTENSITY: f32 = 16.0;
/// The faintest light kept at a lamp's range, and the longest range.
const CUTOFF: f32 = 0.003;
const MAX_RANGE: f32 = 96.0;
/// The near field is softened over at least this distance, so the arm
/// right beside a lamp is not burnt out.
const MIN_SOFTEN: f32 = 8.0;
/// The lamps are a green-leaning cyan; their light is pulled this far
/// toward a clear blue, which reads as their glow on grey surfaces.
const GLOW_BLUE: Vec3 = Vec3::new(0.18, 0.55, 1.0);
const BLUE_SHARE: f32 = 0.45;
/// The light sits this far in front of its lamp.
const SURFACE_OFFSET: f32 = 1.5;
/// Lamps further than this from the view give no light.
const CULL_DISTANCE: f32 = 1500.0;
/// How fast a lamp's light follows whether the view can see it (per
/// second), and how many lamps are checked each frame.
const FADE_RATE: f32 = 5.0;
const CHECKS_PER_FRAME: usize = 48;
/// Lights from lamps closer than this (and facing alike) are merged, and at
/// most this many go to the light list.
const WORLD_MERGE: f32 = 24.0;
const MAX_LAMP_LIGHTS: usize = 24;

pub struct PanelLightsPlugin;

impl Plugin for PanelLightsPlugin {
    fn build(&self, app: &mut App) {
        {
            let mut console = app.world_mut().resource_mut::<portal2_core::console::Console>();
            console.register_cvar("r_fx_panellights", "1", portal2_core::console::cvars::flags::ARCHIVE);
        }
        app.init_resource::<LampCache>().add_systems(PostUpdate, (find_lamps, lamp_lights).chain().in_set(FxProduceSet));
    }
}

/// A lamp in its bone's frame.
#[derive(Clone, Debug, PartialEq)]
struct Lamp {
    bone: usize,
    position: Vec3,
    normal: Vec3,
    /// The far end of a strip.
    end: Option<Vec3>,
    /// Linear colour of unit luminance.
    color: Vec3,
    /// Lit area in square units, weighted by the mask's strength.
    area: f32,
}

/// The lamps found on a model, by model path, skin and body.
#[derive(Resource, Default)]
struct LampCache(HashMap<(String, i32, i32), Arc<Vec<Lamp>>>);

/// A panel model's lamps and how much of each the view sees.
#[derive(Component)]
pub struct PanelLamps {
    key: (usize, i32, i32),
    lamps: Arc<Vec<Lamp>>,
    /// Shown share of each lamp's light, and whether it should be shown.
    seen: Vec<f32>,
    open: Vec<bool>,
}

/// A model checked and found without lamps.
#[derive(Component)]
pub struct NoPanelLamps((usize, i32, i32));

fn model_id(anim: &StudioAnimator) -> usize {
    Arc::as_ptr(&anim.model) as usize
}

fn find_lamps(
    settings: Res<FxSettings>,
    fs: Res<GameFs>,
    mut cache: ResMut<LampCache>,
    models: Query<(Entity, &StudioAnimator, Option<&MapEntity>, Option<&PanelLamps>, Option<&NoPanelLamps>)>,
    mut commands: Commands,
) {
    if !settings.has(features::LIGHTS) {
        return;
    }
    for (e, anim, me, lamps, none) in &models {
        let skin = me.and_then(|m| m.get_i32("skin")).unwrap_or(0);
        let body = me.and_then(|m| m.get_i32("body")).unwrap_or(0);
        let key = (model_id(anim), skin, body);
        if lamps.is_some_and(|l| l.key == key) || none.is_some_and(|n| n.0 == key) {
            continue;
        }
        let path = anim.model.path.to_ascii_lowercase().replace('\\', "/");
        let found = if path.starts_with(MODEL_PREFIX) {
            cache
                .0
                .entry((path, skin, body))
                .or_insert_with(|| Arc::new(lamps_for(&anim.model.studio, &fs, skin.max(0) as usize, body)))
                .clone()
        } else {
            Arc::new(Vec::new())
        };
        if found.is_empty() {
            commands.entity(e).remove::<PanelLamps>().insert(NoPanelLamps(key));
        } else {
            let n = found.len();
            commands.entity(e).remove::<NoPanelLamps>().insert(PanelLamps { key, lamps: found, seen: vec![0.0; n], open: vec![false; n] });
        }
    }
}

/// A material's self-lit mask, with the colour the lit texels show.
struct Mask {
    width: u32,
    height: u32,
    /// Mask strength per texel.
    strength: Vec<f32>,
    /// The base texture (linear) and its size, sampled by texture position.
    base: Vec<Vec3>,
    base_width: u32,
    base_height: u32,
    tint: Vec3,
}

impl Mask {
    fn base_at(&self, uv: Vec2) -> Vec3 {
        let x = ((uv.x.rem_euclid(1.0) * self.base_width as f32) as u32).min(self.base_width - 1);
        let y = ((uv.y.rem_euclid(1.0) * self.base_height as f32) as u32).min(self.base_height - 1);
        self.base[(y * self.base_width + x) as usize]
    }
}

/// The first mip no larger than [`MAX_MASK_SIZE`].
fn small_mip(v: &Vtf) -> u32 {
    let mut mip = 0;
    while mip + 1 < v.mip_count && v.mip_dims(mip).0.max(v.mip_dims(mip).1) > MAX_MASK_SIZE {
        mip += 1;
    }
    mip
}

fn load_mask(fs: &GameFs, vmt_path: &str) -> Option<Mask> {
    let vmt = portal2_render::materials::load_vmt(fs, vmt_path)?;
    if !vmt.get_bool("$selfillum") {
        return None;
    }
    let base = Vtf::parse(&fs.read(&vmt.texture_path("$basetexture")?)?).ok()?;
    let base_mip = small_mip(&base);
    let (base_width, base_height, _) = base.mip_dims(base_mip);
    let base_rgba = base.decode_rgba8(base_mip, 0, 0);
    let base_rgb: Vec<Vec3> = base_rgba
        .chunks_exact(4)
        .map(|p| Vec3::new(srgb_to_linear(p[0] as f32 / 255.0), srgb_to_linear(p[1] as f32 / 255.0), srgb_to_linear(p[2] as f32 / 255.0)))
        .collect();
    let (width, height, strength) = match vmt.texture_path("$selfillummask").and_then(|p| fs.read(&p)).and_then(|d| Vtf::parse(&d).ok()) {
        // A separate mask lights by its colour channels.
        Some(mask) => {
            let mip = small_mip(&mask);
            let (w, h, _) = mask.mip_dims(mip);
            let s = mask.decode_rgba8(mip, 0, 0).chunks_exact(4).map(|p| p[0].max(p[1]).max(p[2]) as f32 / 255.0).collect();
            (w, h, s)
        }
        // Otherwise the base texture's alpha; without one the whole
        // surface glows, which is no lamp.
        None if base.format.has_alpha() => (base_width, base_height, base_rgba.chunks_exact(4).map(|p| p[3] as f32 / 255.0).collect()),
        None => return None,
    };
    let tint = vmt.get_vec3("$selfillumtint").unwrap_or(Vec3::ONE);
    Some(Mask { width, height, strength, base: base_rgb, base_width, base_height, tint })
}

/// A lit blob of a mask, in texture coordinates.
#[derive(Clone, Debug, PartialEq)]
struct Blob {
    uv: Vec2,
    /// Its weighted area as a share of the whole texture.
    share: f32,
    /// Linear colour times the tint (not normalised).
    color: Vec3,
    /// The ends of a strip.
    ends: Option<(Vec2, Vec2)>,
}

/// The lit blobs of a mask (8-connected texels above the threshold).
fn find_blobs(mask: &Mask) -> Vec<Blob> {
    let (w, h) = (mask.width as i32, mask.height as i32);
    let lit = mask.strength.iter().filter(|&&s| s > MASK_THRESHOLD).count();
    if lit as f32 > mask.strength.len() as f32 * MAX_LIT_SHARE {
        return Vec::new();
    }
    let mut visited = vec![false; mask.strength.len()];
    let mut blobs = Vec::new();
    let mut stack = Vec::new();
    for start in 0..mask.strength.len() {
        if visited[start] || mask.strength[start] <= MASK_THRESHOLD {
            continue;
        }
        visited[start] = true;
        stack.push(start);
        let (mut weight, mut sum, mut sum2, mut color) = (0.0f32, Vec2::ZERO, Vec3::ZERO, Vec3::ZERO);
        let mut texels: Vec<(Vec2, f32)> = Vec::new();
        while let Some(i) = stack.pop() {
            let (x, y) = (i as i32 % w, i as i32 / w);
            let s = mask.strength[i];
            let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
            weight += s;
            sum += p * s;
            sum2 += Vec3::new(p.x * p.x, p.y * p.y, p.x * p.y) * s;
            let uv = p / Vec2::new(w as f32, h as f32);
            color += mask.base_at(uv) * s;
            texels.push((p, s));
            for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w || ny >= h {
                    continue;
                }
                let j = (ny * w + nx) as usize;
                if !visited[j] && mask.strength[j] > MASK_THRESHOLD {
                    visited[j] = true;
                    stack.push(j);
                }
            }
        }
        if weight < MIN_BLOB_TEXELS {
            continue;
        }
        let mean = sum / weight;
        // Spread along the blob's main axis, from its covariance.
        let (cxx, cyy, cxy) = (sum2.x / weight - mean.x * mean.x, sum2.y / weight - mean.y * mean.y, sum2.z / weight - mean.x * mean.y);
        let half_trace = (cxx + cyy) * 0.5;
        let spread = (half_trace * half_trace - (cxx * cyy - cxy * cxy)).max(0.0).sqrt();
        let (major, minor) = (half_trace + spread, (half_trace - spread).max(0.0));
        let ends = (major > STRIP_ASPECT * STRIP_ASPECT * minor.max(0.25) && major.sqrt() > 3.0).then(|| {
            let dir = if cxy.abs() > 1e-6 {
                Vec2::new(major - cyy, cxy).normalize()
            } else if cxx >= cyy {
                Vec2::X
            } else {
                Vec2::Y
            };
            // A uniform line of length L spreads L^2 / 12 along itself.
            let half = (3.0 * major).sqrt();
            let size = Vec2::new(w as f32, h as f32);
            ((mean - dir * half) / size, (mean + dir * half) / size)
        });
        blobs.push(Blob {
            uv: mean / Vec2::new(w as f32, h as f32),
            share: weight / (w * h) as f32,
            color: color / weight * mask.tint,
            ends,
        });
    }
    blobs
}

/// Barycentric weights of `p` in the texture-space triangle, if inside
/// (allowing for coordinates tiled past 0..1).
fn barycentric(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> Option<Vec3> {
    let shift = a.min(b).min(c).floor();
    let p = p + shift;
    let (v0, v1, v2) = (b - a, c - a, p - a);
    let det = v0.perp_dot(v1);
    if det.abs() < 1e-12 {
        return None;
    }
    let v = v2.perp_dot(v1) / det;
    let w = v0.perp_dot(v2) / det;
    let u = 1.0 - v - w;
    const EPS: f32 = -1e-4;
    (u >= EPS && v >= EPS && w >= EPS).then_some(Vec3::new(u, v, w))
}

/// Where a texture position lies on a mesh: model-space point, face normal,
/// the bone it follows, and square units per unit of texture area.
fn locate(mesh: &source_formats::studio::StudioMesh, uv: Vec2) -> Vec<(Vec3, Vec3, usize, f32)> {
    let mut hits = Vec::new();
    for t in mesh.indices.chunks_exact(3) {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| &mesh.vertices[i as usize]);
        let Some(bary) = barycentric(uv, a.uv, b.uv, c.uv) else { continue };
        let pos = a.pos * bary.x + b.pos * bary.y + c.pos * bary.z;
        let cross = (b.pos - a.pos).cross(c.pos - a.pos);
        let mut normal = cross.normalize_or_zero();
        // Face the way the vertex normals do.
        if normal.dot(a.normal + b.normal + c.normal) < 0.0 {
            normal = -normal;
        }
        let uv_area = (b.uv - a.uv).perp_dot(c.uv - a.uv).abs() * 0.5;
        if uv_area < 1e-10 || normal == Vec3::ZERO {
            continue;
        }
        let scale = cross.length() * 0.5 / uv_area;
        // The bone with the most weight over the triangle's corners.
        let mut weights: Vec<(usize, f32)> = Vec::new();
        for v in [a, b, c] {
            for k in 0..v.num_bones.max(1) as usize {
                let bone = v.bones[k] as usize;
                match weights.iter_mut().find(|(b, _)| *b == bone) {
                    Some(w) => w.1 += v.weights[k],
                    None => weights.push((bone, v.weights[k])),
                }
            }
        }
        let bone = weights.iter().max_by(|x, y| x.1.total_cmp(&y.1)).map_or(0, |w| w.0);
        hits.push((pos, normal, bone, scale));
    }
    hits
}

fn unit_luminance(c: Vec3) -> Vec3 {
    c / c.dot(Vec3::new(0.2126, 0.7152, 0.0722)).max(1e-4)
}

/// A lamp's light colour from its texels: cyan lamps lean blue, other
/// colours (an error skin's red) stay their own.
fn glow_color(texels: Vec3) -> Vec3 {
    let c = unit_luminance(texels);
    let cyan = (c.y.min(c.z) - c.x).max(0.0) / c.max_element().max(1e-4);
    unit_luminance(c.lerp(unit_luminance(GLOW_BLUE), BLUE_SHARE * cyan))
}

/// The lamps of a model with a skin and body.
fn lamps_for(studio: &StudioModel, fs: &GameFs, skin: usize, body: i32) -> Vec<Lamp> {
    let materials = portal2_render::models::skin_materials(studio, fs, skin);
    let bodies = portal2_render::models::body_selection(studio, body);
    let mut masks: HashMap<String, Option<(Mask, Vec<Blob>)>> = HashMap::new();
    let mut lamps: Vec<Lamp> = Vec::new();
    for mesh in &studio.meshes {
        if bodies.get(mesh.body_part).copied().unwrap_or(0) != mesh.model {
            continue;
        }
        let Some(path) = materials.get(mesh.material).cloned().flatten() else { continue };
        let entry = masks.entry(path.clone()).or_insert_with(|| {
            load_mask(fs, &path).map(|m| {
                let blobs = find_blobs(&m);
                (m, blobs)
            })
        });
        let Some((_, blobs)) = entry else { continue };
        for blob in blobs.iter() {
            for (pos, normal, bone, scale) in locate(mesh, blob.uv) {
                let Some(b) = studio.mdl.bones.get(bone) else { continue };
                let to_bone = b.pose_to_bone;
                // A strip's ends, when both lie on the same bone.
                let end = blob.ends.and_then(|(u0, u1)| {
                    let near = |uv: Vec2| {
                        locate(mesh, uv).into_iter().filter(|h| h.2 == bone).min_by(|x, y| x.0.distance(pos).total_cmp(&y.0.distance(pos)))
                    };
                    let (p0, p1) = (near(u0)?, near(u1)?);
                    Some((p0.0, p1.0))
                });
                let (start, end) = match end {
                    Some((p0, p1)) => (p0, Some(to_bone.transform_point3(p1))),
                    None => (pos, None),
                };
                lamps.push(Lamp {
                    bone,
                    position: to_bone.transform_point3(start),
                    normal: to_bone.transform_vector3(normal).normalize_or_zero(),
                    end,
                    color: blob.color,
                    area: blob.share * scale,
                });
            }
        }
    }
    let mut merged = merge_on_bones(lamps);
    for l in &mut merged {
        l.color = glow_color(l.color);
    }
    merged.retain(|l| l.area > 0.0 && l.normal != Vec3::ZERO);
    if !merged.is_empty() {
        debug!("{} panel lamps on {}", merged.len(), studio.path);
    }
    merged
}

/// Lamps on one bone that sit close together (the same blob reached through
/// overlapping triangles, a lamp's mirrored halves) become one.
fn merge_on_bones(lamps: Vec<Lamp>) -> Vec<Lamp> {
    let mut out: Vec<Lamp> = Vec::new();
    for l in lamps {
        // The same strip found again.
        if let Some(end) = l.end
            && let Some(m) = out.iter_mut().find(|m| {
                m.bone == l.bone
                    && m.end.is_some_and(|e| e.distance(end) < MERGE_ON_BONE)
                    && m.position.distance(l.position) < MERGE_ON_BONE
            })
        {
            m.area = m.area.max(l.area);
            continue;
        }
        if l.end.is_none()
            && let Some(m) = out.iter_mut().find(|m| m.end.is_none() && m.bone == l.bone && m.position.distance(l.position) < MERGE_ON_BONE)
        {
            let total = m.area + l.area;
            m.position = (m.position * m.area + l.position * l.area) / total.max(1e-6);
            m.normal = (m.normal * m.area + l.normal * l.area).normalize_or(m.normal);
            m.color = (m.color * m.area + l.color * l.area) / total.max(1e-6);
            m.area = total;
            continue;
        }
        out.push(l);
    }
    out
}

/// A lamp's light at full strength, and how far it reaches.
fn lamp_intensity(area: f32) -> (f32, f32) {
    let intensity = (area * INTENSITY_PER_AREA).clamp(MIN_INTENSITY, MAX_INTENSITY);
    (intensity, (intensity / CUTOFF).sqrt().min(MAX_RANGE))
}

#[allow(clippy::too_many_arguments)]
fn lamp_lights(
    settings: Res<FxSettings>,
    console: Res<portal2_core::console::Console>,
    time: Res<Time>,
    physics: portal2_world::collision::Physics,
    camera: Query<&GlobalTransform, With<MainCamera>>,
    mut models: Query<(Entity, &mut PanelLamps, &StudioAnimator, Option<&InheritedVisibility>)>,
    joints: Query<&GlobalTransform>,
    mut next_check: Local<usize>,
    mut lights: ResMut<FxLights>,
) {
    let scale = console.f32("r_fx_panellights");
    if !settings.has(features::LIGHTS) || scale <= 0.0 {
        return;
    }
    let Ok(eye) = camera.single().map(|c| c.translation()) else { return };
    let fade = (FADE_RATE * time.delta_secs()).min(1.0);
    // The lamps' visibility is re-checked a few at a time, in turn.
    let total: usize = models.iter().map(|(_, l, _, _)| l.lamps.len()).sum();
    let (first, last) = (*next_check, *next_check + CHECKS_PER_FRAME.min(total));
    *next_check = if total > 0 { last % total } else { 0 };
    let due = |i: usize| (i >= first && i < last) || (last > total && i < last - total);
    let mut index = 0;
    let mut out: Vec<FxLightData> = Vec::new();
    for (e, mut lamps, anim, shown) in &mut models {
        let hidden = shown.is_some_and(|v| !v.get());
        let lamps = &mut *lamps;
        for (i, lamp) in lamps.lamps.iter().enumerate() {
            let k = index;
            index += 1;
            let joint = anim.joints.get(lamp.bone).and_then(|j| joints.get(*j).ok());
            let Some(joint) = joint.filter(|_| !hidden) else {
                lamps.seen[i] = 0.0;
                continue;
            };
            let normal = joint.affine().transform_vector3(lamp.normal).normalize_or_zero();
            let position = joint.transform_point(lamp.position) + normal * SURFACE_OFFSET;
            if position.distance(eye) > CULL_DISTANCE {
                lamps.seen[i] = 0.0;
                lamps.open[i] = false;
                continue;
            }
            if due(k) {
                let t = physics.trace(eye, position, Vec3::ZERO, Vec3::ZERO, portal2_world::collision::mask::SOLID_ALL, &[e]);
                lamps.open[i] = t.fraction >= 0.999 && !t.start_solid;
            }
            let goal = if lamps.open[i] { 1.0 } else { 0.0 };
            lamps.seen[i] += (goal - lamps.seen[i]) * fade;
            if lamps.seen[i] < 0.01 {
                continue;
            }
            let (intensity, range) = lamp_intensity(lamp.area);
            let radius = (lamp.area / std::f32::consts::PI).sqrt().max(0.75);
            let color = lamp.color * intensity * lamps.seen[i] * scale;
            let light = match lamp.end {
                Some(end) => FxLightData {
                    position,
                    shape: FxLightShape::Line { end: joint.transform_point(end) + normal * SURFACE_OFFSET },
                    color,
                    range,
                    flags: light_flags::DIFFUSE | light_flags::SPECULAR,
                    radius: radius.max(MIN_SOFTEN),
                    priority: 3.0,
                    shadow: false,
                },
                None => FxLightData {
                    position,
                    // Over the lamp's front, a little round its edge.
                    shape: FxLightShape::Spot {
                        axis: normal,
                        cos_inner: 1.0,
                        cos_outer: -0.2,
                        exponent: 1.0,
                        attenuation: Vec3::new((radius * radius * 8.0).max(MIN_SOFTEN * MIN_SOFTEN), 0.0, 1.0),
                    },
                    color,
                    range,
                    flags: light_flags::DIFFUSE | light_flags::SPECULAR,
                    radius,
                    priority: 3.0,
                    shadow: false,
                },
            };
            out.push(light);
        }
    }
    let mut merged = merge_in_world(out);
    // The ones that matter most to the view.
    let luma = |l: &FxLightData| l.color.dot(Vec3::new(0.299, 0.587, 0.114));
    merged.sort_by(|a, b| {
        let sa = luma(a) / (a.position.distance_squared(eye) + 4096.0);
        let sb = luma(b) / (b.position.distance_squared(eye) + 4096.0);
        sb.total_cmp(&sa)
    });
    merged.truncate(MAX_LAMP_LIGHTS);
    lights.frame.extend(merged);
}

/// Lamp lights close together (a row of arms side by side) become one, at
/// their brightness-weighted middle.
fn merge_in_world(lights: Vec<FxLightData>) -> Vec<FxLightData> {
    let luma = |l: &FxLightData| l.color.dot(Vec3::new(0.299, 0.587, 0.114)).max(1e-6);
    let mut sorted = lights;
    sorted.sort_by(|a, b| luma(b).total_cmp(&luma(a)));
    let mut out: Vec<(FxLightData, f32)> = Vec::new();
    for l in sorted {
        if let FxLightShape::Spot { axis, .. } = l.shape {
            let found = out.iter_mut().find(|(m, _)| match m.shape {
                FxLightShape::Spot { axis: other, .. } => m.position.distance(l.position) < WORLD_MERGE && axis.dot(other) > 0.5,
                _ => false,
            });
            if let Some((m, weight)) = found {
                let w = luma(&l);
                m.position = (m.position * *weight + l.position * w) / (*weight + w);
                m.color += l.color;
                m.range = m.range.max(l.range);
                m.radius = m.radius.max(l.radius);
                *weight += w;
                continue;
            }
        }
        let w = luma(&l);
        out.push((l, w));
    }
    out.into_iter().map(|(l, _)| l).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask(width: u32, height: u32, lit: &[(u32, u32, f32)]) -> Mask {
        let mut strength = vec![0.0; (width * height) as usize];
        for &(x, y, s) in lit {
            strength[(y * width + x) as usize] = s;
        }
        Mask {
            width,
            height,
            strength,
            base: vec![Vec3::new(0.0, 0.8, 0.7); (width * height) as usize],
            base_width: width,
            base_height: height,
            tint: Vec3::ONE,
        }
    }

    #[test]
    fn finds_dots_and_strips() {
        // A 2x2 dot and a 1x12 strip.
        let mut lit = vec![(4, 4, 1.0), (5, 4, 1.0), (4, 5, 1.0), (5, 5, 1.0)];
        lit.extend((10..22).map(|y| (30, y, 1.0)));
        let blobs = find_blobs(&mask(64, 64, &lit));
        assert_eq!(blobs.len(), 2);
        let dot = &blobs[0];
        assert!((dot.uv - Vec2::new(5.0 / 64.0, 5.0 / 64.0)).length() < 1e-5, "{:?}", dot.uv);
        assert!((dot.share - 4.0 / 4096.0).abs() < 1e-7);
        assert!(dot.ends.is_none());
        let strip = &blobs[1];
        let (a, b) = strip.ends.expect("a strip");
        assert!((a.x - 30.5 / 64.0).abs() < 1e-4 && (b.x - 30.5 / 64.0).abs() < 1e-4);
        assert!(((a.y - b.y).abs() * 64.0 - 12.0).abs() < 0.5, "{a:?} {b:?}");
        // Faint texels are not lamps, and a mostly lit mask has none.
        assert!(find_blobs(&mask(8, 8, &[(1, 1, 0.2)])).is_empty());
        let all: Vec<(u32, u32, f32)> = (0..64).map(|i| (i % 8, i / 8, 1.0)).collect();
        assert!(find_blobs(&mask(8, 8, &all)).is_empty());
    }

    #[test]
    fn barycentric_handles_tiling() {
        let (a, b, c) = (Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0));
        assert!(barycentric(Vec2::new(0.2, 0.2), a, b, c).is_some());
        assert!(barycentric(Vec2::new(0.8, 0.8), a, b, c).is_none());
        // The same triangle tiled one texture over.
        let s = Vec2::X;
        let w = barycentric(Vec2::new(0.2, 0.2), a + s, b + s, c + s).expect("inside");
        assert!((w - Vec3::new(0.6, 0.2, 0.2)).length() < 1e-5);
    }

    #[test]
    fn close_lamps_merge() {
        let lamp =
            |x: f32, area: f32| Lamp { bone: 1, position: Vec3::new(x, 0.0, 0.0), normal: Vec3::Z, end: None, color: Vec3::ONE, area };
        let merged = merge_on_bones(vec![lamp(0.0, 1.0), lamp(2.0, 3.0), lamp(20.0, 1.0)]);
        assert_eq!(merged.len(), 2);
        assert!((merged[0].position.x - 1.5).abs() < 1e-5 && merged[0].area == 4.0);
        // Brighter lamps reach further, within bounds.
        let (small, small_range) = lamp_intensity(0.1);
        let (big, big_range) = lamp_intensity(100.0);
        assert_eq!((small, big), (MIN_INTENSITY, MAX_INTENSITY));
        assert!(small_range < big_range && big_range <= MAX_RANGE);
    }
}
