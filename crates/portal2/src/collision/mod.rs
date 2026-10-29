//! Collision: static map geometry in a BVH, entity colliders, and swept-box
//! traces across both.

pub mod build;
pub mod bvh;
pub mod volume;

use std::sync::Arc;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use source_formats::bsp::contents;

use crate::vfs::GameFs;
use crate::world::{CurrentMap, MapLoaded};
use bvh::Bvh;
use volume::ConvexVolume;

pub mod mask {
    use source_formats::bsp::contents::*;
    pub const PLAYER_SOLID: u32 = SOLID | MOVEABLE | PLAYERCLIP | WINDOW | MONSTER | GRATE;
    pub const SOLID_ALL: u32 = SOLID | MOVEABLE | WINDOW | MONSTER | GRATE;
    /// World and brush geometry, not models or characters.
    pub const SOLID_BRUSH_ONLY: u32 = SOLID | MOVEABLE | WINDOW | GRATE;
    pub const ALL: u32 = u32::MAX;
}

pub struct CollisionPlugin;

impl Plugin for CollisionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CollisionWorld>()
            .add_systems(PreUpdate, rebuild_on_map_load.after(crate::world::LoadMapSet).before(crate::entities::EntitySpawnSet));
    }
}

/// Collision shape in its own space.
#[derive(Clone, Debug, Default)]
pub struct Shape {
    pub volumes: Vec<ConvexVolume>,
    pub mins: Vec3,
    pub maxs: Vec3,
}

impl Shape {
    pub fn new(volumes: Vec<ConvexVolume>) -> Self {
        let mut mins = Vec3::splat(f32::MAX);
        let mut maxs = Vec3::splat(f32::MIN);
        for v in &volumes {
            mins = mins.min(v.mins);
            maxs = maxs.max(v.maxs);
        }
        if volumes.is_empty() {
            mins = Vec3::ZERO;
            maxs = Vec3::ZERO;
        }
        Self { volumes, mins, maxs }
    }
}

#[derive(Resource, Default)]
pub struct CollisionWorld {
    pub statics: Vec<ConvexVolume>,
    bvh: Bvh,
    /// Shapes of brush models (index 0, the world, lives in `statics`).
    pub brush_models: Vec<Arc<Shape>>,
}

impl CollisionWorld {
    pub fn from_statics(statics: Vec<ConvexVolume>) -> Self {
        let bounds: Vec<(Vec3, Vec3)> = statics.iter().map(|v| (v.mins, v.maxs)).collect();
        Self { bvh: Bvh::build(&bounds), statics, brush_models: Vec::new() }
    }

    /// Indices of static volumes whose bounds overlap a box.
    pub fn statics_in(&self, mins: Vec3, maxs: Vec3) -> Vec<u32> {
        let mut out = Vec::new();
        self.bvh.query_aabb(mins, maxs, |i| out.push(i));
        out
    }
}

/// Collision attached to an entity; volumes are in the entity's space.
#[derive(Component, Clone)]
pub struct Collider {
    pub shape: Arc<Shape>,
    pub enabled: bool,
    /// Whether the volumes block movement (false for triggers, which only
    /// use the shape for touch tests).
    pub solid: bool,
}

pub fn rebuild_on_map_load(
    mut loaded: MessageReader<MapLoaded>,
    map: Option<Res<CurrentMap>>,
    fs: Res<GameFs>,
    mut cw: ResMut<CollisionWorld>,
) {
    if loaded.read().last().is_none() {
        return;
    }
    let Some(map) = map else { return };
    let started = std::time::Instant::now();
    let bsp = &map.bsp;
    let mut statics = build::model_volumes(bsp, 0);
    let brushes = statics.len();
    let disp = build::displacement_volumes(bsp);
    let disp_count = disp.len();
    statics.extend(disp);
    let props = build::static_prop_volumes(bsp, &fs);
    let prop_count = props.len();
    statics.extend(props);
    let bounds: Vec<(Vec3, Vec3)> = statics.iter().map(|v| (v.mins, v.maxs)).collect();
    let bvh = Bvh::build(&bounds);
    let brush_models = (0..bsp.models.len())
        .map(|m| if m == 0 { Arc::new(Shape::default()) } else { Arc::new(Shape::new(build::model_volumes(bsp, m))) })
        .collect();
    info!("collision: {brushes} brushes, {disp_count} displacement triangles, {prop_count} prop hulls in {:?}", started.elapsed());
    *cw = CollisionWorld { statics, bvh, brush_models };
}

#[derive(Clone, Copy, Debug)]
pub struct Trace {
    pub fraction: f32,
    pub end: Vec3,
    pub normal: Vec3,
    pub start_solid: bool,
    pub all_solid: bool,
    pub contents: u32,
    pub entity: Option<Entity>,
    pub surface: i32,
}

impl Trace {
    fn empty(end: Vec3) -> Self {
        Self { fraction: 1.0, end, normal: Vec3::ZERO, start_solid: false, all_solid: false, contents: 0, entity: None, surface: -1 }
    }

    pub fn hit(&self) -> bool {
        self.fraction < 1.0 || self.start_solid
    }
}

/// Collision queries for systems.
#[derive(SystemParam)]
pub struct Physics<'w, 's> {
    pub world: Res<'w, CollisionWorld>,
    colliders: Query<'w, 's, (Entity, &'static Collider, &'static GlobalTransform)>,
    holes: Option<Res<'w, crate::portals::PortalHoles>>,
}

impl Physics<'_, '_> {
    /// Sweeps the box `[mins, maxs]` from `start` to `end` against everything
    /// whose contents intersect `mask`, ignoring entities in `ignore`.
    pub fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32, ignore: &[Entity]) -> Trace {
        let mut best = Trace::empty(end);
        let mut consider = |hit: volume::VolumeHit, contents: u32, entity: Option<Entity>, normal: Vec3, best: &mut Trace| {
            if hit.start_solid {
                best.start_solid = true;
                if hit.all_solid {
                    best.all_solid = true;
                }
                if best.fraction > 0.0 || best.entity.is_none() {
                    best.fraction = 0.0;
                    best.normal = normal;
                    best.contents = contents;
                    best.entity = entity;
                    best.surface = hit.plane.surface;
                }
            } else if hit.fraction < best.fraction {
                best.fraction = hit.fraction;
                best.normal = normal;
                best.contents = contents;
                best.entity = entity;
                best.surface = hit.plane.surface;
            }
        };

        // A box inside a linked portal's opening passes through the wall the
        // portal is on, but is held in the tunnel by the opening's frame.
        let in_holes: Vec<&crate::portals::PortalHole> = match &self.holes {
            Some(h) if mins != maxs => {
                h.holes.iter().filter(|hole| hole.contains_box(start, mins, maxs) || hole.contains_box(end, mins, maxs)).collect()
            }
            _ => Vec::new(),
        };
        self.world.bvh.query_sweep(start, end, mins, maxs, |i| {
            let v = &self.world.statics[i as usize];
            if v.contents & mask == 0 {
                return;
            }
            if in_holes.iter().any(|h| h.carves(i)) {
                return;
            }
            if let Some(hit) = v.sweep(start, end, mins, maxs) {
                consider(hit, v.contents, None, hit.plane.normal, &mut best);
            }
        });
        for hole in &in_holes {
            for v in &hole.frame_volumes {
                if let Some(hit) = v.sweep(start, end, mins, maxs) {
                    consider(hit, v.contents, None, hit.plane.normal, &mut best);
                }
            }
        }

        for (e, col, gt) in &self.colliders {
            if !col.enabled || !col.solid || ignore.contains(&e) {
                continue;
            }
            let affine = gt.affine();
            let inv = affine.inverse();
            let (ls, le) = (inv.transform_point3(start), inv.transform_point3(end));
            let (lmins, lmaxs) = rotated_box(inv.matrix3.into(), mins, maxs);
            let delta = le - ls;
            if !bvh::segment_hits_aabb(ls, delta, col.shape.mins - lmaxs - Vec3::ONE, col.shape.maxs - lmins + Vec3::ONE) {
                continue;
            }
            for v in &col.shape.volumes {
                if v.contents & mask == 0 {
                    continue;
                }
                // Brush entities a portal sits on are cut like the world.
                if !in_holes.is_empty() {
                    let corners: Vec<Vec3> = (0..8)
                        .map(|i| {
                            let c = Vec3::new(
                                if i & 1 == 0 { v.mins.x } else { v.maxs.x },
                                if i & 2 == 0 { v.mins.y } else { v.maxs.y },
                                if i & 4 == 0 { v.mins.z } else { v.maxs.z },
                            );
                            affine.transform_point3(c)
                        })
                        .collect();
                    if in_holes.iter().any(|h| h.carves_bounds(&corners)) {
                        continue;
                    }
                }
                if let Some(hit) = v.sweep(ls, le, lmins, lmaxs) {
                    let n = Vec3::from(affine.matrix3 * bevy::math::Vec3A::from(hit.plane.normal)).normalize_or_zero();
                    consider(hit, v.contents, Some(e), n, &mut best);
                }
            }
        }
        best.end = start + (end - start) * best.fraction;
        best
    }

    /// Entities (and the world, as `None`) whose solid volumes overlap a box.
    pub fn overlapping(&self, pos: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> Vec<Option<Entity>> {
        let mut out = Vec::new();
        let mut world_hit = false;
        self.world.bvh.query_aabb(pos + mins, pos + maxs, |i| {
            let v = &self.world.statics[i as usize];
            if !world_hit && v.contents & mask != 0 && v.overlaps_box(pos, mins, maxs) {
                world_hit = true;
            }
        });
        if world_hit {
            out.push(None);
        }
        for (e, col, gt) in &self.colliders {
            if !col.enabled || !col.solid {
                continue;
            }
            let inv = gt.affine().inverse();
            let lp = inv.transform_point3(pos);
            let (lmins, lmaxs) = rotated_box(inv.matrix3.into(), mins, maxs);
            if col.shape.volumes.iter().any(|v| v.contents & mask != 0 && v.overlaps_box(lp, lmins, lmaxs)) {
                out.push(Some(e));
            }
        }
        out
    }

    /// Whether a box overlaps a specific entity's shape (solid or not).
    pub fn touches(&self, entity: Entity, pos: Vec3, mins: Vec3, maxs: Vec3) -> bool {
        let Ok((_, col, gt)) = self.colliders.get(entity) else { return false };
        if !col.enabled {
            return false;
        }
        let inv = gt.affine().inverse();
        let lp = inv.transform_point3(pos);
        let (lmins, lmaxs) = rotated_box(inv.matrix3.into(), mins, maxs);
        col.shape.volumes.iter().any(|v| v.overlaps_box(lp, lmins, lmaxs))
    }
}

/// Axis-aligned bounds of a box after rotating it by `m`.
fn rotated_box(m: Mat3, mins: Vec3, maxs: Vec3) -> (Vec3, Vec3) {
    if m.abs_diff_eq(Mat3::IDENTITY, 1e-5) {
        return (mins, maxs);
    }
    let c = (mins + maxs) * 0.5;
    let e = (maxs - mins) * 0.5;
    let rc = m * c;
    let re = Vec3::new(
        m.x_axis.x.abs() * e.x + m.y_axis.x.abs() * e.y + m.z_axis.x.abs() * e.z,
        m.x_axis.y.abs() * e.x + m.y_axis.y.abs() * e.y + m.z_axis.y.abs() * e.z,
        m.x_axis.z.abs() * e.x + m.y_axis.z.abs() * e.y + m.z_axis.z.abs() * e.z,
    );
    (rc - re, rc + re)
}

/// Whether a brush entity class blocks movement.
pub fn brush_class_is_solid(class: &str) -> bool {
    !(class.starts_with("trigger_")
        || matches!(
            class,
            "func_illusionary"
                | "func_areaportal"
                | "func_areaportalwindow"
                | "func_occluder"
                | "func_clip_vphysics"
                | "func_noportal_volume"
                | "func_portal_bumper"
                | "func_portal_detector"
                | "func_portal_orientation"
                | "func_placement_clip"
                | "func_precipitation"
                | "func_vehicleclip"
                | "func_dustcloud"
                | "func_smokevolume"
                | "func_instance_io_proxy"
                | "env_bubbles"
        ))
}

/// The world's own solid contents flag, re-exported for callers.
pub const SOLID: u32 = contents::SOLID;
