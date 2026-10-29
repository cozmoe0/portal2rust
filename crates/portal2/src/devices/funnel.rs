//! Excursion funnels: a projector casts a tube that carries the player and
//! props along its axis (reversible), continuing through linked portals.

use avian3d::prelude::{GravityScale, LinearVelocity, RigidBody};
use bevy::prelude::*;
use source_formats::bsp::contents;

use super::{BeamSegment, EffectMaterial, linked_frames, segment_transform, trace_beam, tube_mesh, with_ancestors};
use crate::collision::{Collider as TraceCollider, Physics};
use crate::entities::io::{InputCall, InputHandlers};
use crate::entities::props::PhysicsProp;
use crate::entities::spawner::{ModelOptions, attach_studio_model};
use crate::entities::{MapEntity, SpawnHandlers};
use crate::player::{Player, PlayerMoveSet};
use crate::portals::{Portal, Portals};
use crate::world::MapObject;

const FUNNEL_MASK: u32 = contents::SOLID | contents::MOVEABLE | contents::WINDOW;
const DEFAULT_RADIUS: f32 = 54.0;
/// How hard objects are pulled back toward the funnel's axis.
const CENTERING: f32 = 3.0;
/// Sideways speed the player can steer with inside a funnel.
const STEER_SPEED: f32 = 120.0;

pub struct FunnelPlugin;

impl Plugin for FunnelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlayerPull>().add_systems(FixedUpdate, update_funnels.before(PlayerMoveSet)).add_systems(Update, draw_funnels);
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("prop_tractor_beam", spawn_funnel);
        world.resource_mut::<InputHandlers>().register("prop_tractor_beam", funnel_input);
    }
}

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct Funnel {
    pub enabled: bool,
    /// Signed speed along the beam (negative pulls toward the projector).
    pub force: f32,
    pub radius: f32,
    #[serde(skip)]
    pub segments: Vec<BeamSegment>,
    #[serde(skip)]
    tubes: Vec<Entity>,
}

/// What a funnel is doing to the player this tick.
#[derive(Clone, Copy, Debug)]
pub struct Pull {
    pub dir: Vec3,
    pub axis_point: Vec3,
    pub speed: f32,
}

#[derive(Resource, Default)]
pub struct PlayerPull(pub Option<Pull>);

/// Props currently carried by a funnel (gravity switched off).
#[derive(Component)]
struct InFunnel;

fn spawn_funnel(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let model = me.studio_model().and_then(|p| attach_studio_model(world, e, &p, ModelOptions::default()));
    let radius = model
        .map(|m| {
            let mdl = &m.studio.mdl;
            let lo = mdl.hull_min.min(mdl.view_bbmin);
            let hi = mdl.hull_max.max(mdl.view_bbmax);
            (hi.y - lo.y).abs().min((hi.z - lo.z).abs()) * 0.5
        })
        .filter(|r| (24.0..=128.0).contains(r))
        .unwrap_or(DEFAULT_RADIUS);
    world.entity_mut(e).insert(Funnel {
        enabled: me.get_i32("startenabled").unwrap_or(1) != 0,
        force: me.get_f32("linearforce").unwrap_or(250.0),
        radius,
        segments: Vec::new(),
        tubes: Vec::new(),
    });
}

fn funnel_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut f) = world.get_mut::<Funnel>(call.target) else { return false };
    match call.input.as_str() {
        "enable" => f.enabled = true,
        "disable" => f.enabled = false,
        "toggle" => f.enabled = !f.enabled,
        "setlinearforce" => f.force = source_formats::kv::parse_f32_lenient(&call.param),
        _ => return false,
    }
    true
}

/// Velocity a funnel wants the player to have. Steering (`wish`, a unit-ish
/// direction from the view) slackens the centering so the player can drift
/// out of the side of the tube.
pub fn player_flow(pull: &Pull, center: Vec3, wish: Vec3) -> Vec3 {
    let steer = (wish - pull.dir * wish.dot(pull.dir)).normalize_or_zero() * STEER_SPEED;
    let centering = if steer == Vec3::ZERO { CENTERING } else { CENTERING * 0.25 };
    pull.dir * pull.speed + (pull.axis_point - center) * centering + steer
}

/// Where `p` sits relative to the funnel: the pull to apply, if inside.
fn pull_at(f: &Funnel, p: Vec3) -> Option<Pull> {
    for seg in &f.segments {
        let d = seg.end - seg.start;
        let len = d.length();
        if len < 1.0 {
            continue;
        }
        let dir = d / len;
        let t = (p - seg.start).dot(dir);
        if !(0.0..=len).contains(&t) {
            continue;
        }
        let axis_point = seg.start + dir * t;
        if axis_point.distance(p) <= f.radius {
            let sign = if f.force < 0.0 { -1.0 } else { 1.0 };
            return Some(Pull { dir: dir * sign, axis_point, speed: f.force.abs() });
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn update_funnels(
    mut commands: Commands,
    physics: Physics,
    portals: Res<Portals>,
    portal_query: Query<&Portal>,
    mut funnels: Query<(Entity, &mut Funnel, &GlobalTransform)>,
    parents: Query<&ChildOf>,
    players: Query<&Player>,
    mut player_pull: ResMut<PlayerPull>,
    mut props: Query<(Entity, &TraceCollider, &GlobalTransform, &RigidBody, &mut LinearVelocity, Has<InFunnel>), With<PhysicsProp>>,
    holding: Option<Res<crate::physics::carry::Holding>>,
) {
    let frames = linked_frames(&portals, &portal_query);
    for (e, mut f, gt) in &mut funnels {
        if !f.enabled {
            f.segments.clear();
            continue;
        }
        let t = gt.compute_transform();
        let fwd = t.rotation * Vec3::X;
        let ignore = with_ancestors(e, &parents);
        f.segments = trace_beam(&physics, frames, t.translation + fwd * 8.0, fwd, FUNNEL_MASK, &ignore, 8, |_| None);
    }

    player_pull.0 = None;
    if let Ok(p) = players.single()
        && !p.noclip
    {
        let (mins, maxs) = p.state.hull();
        let center = p.state.origin + (mins + maxs) * 0.5;
        player_pull.0 = funnels.iter().find_map(|(_, f, _)| pull_at(&f, center));
    }

    let held = holding.and_then(|h| h.entity);
    for (pe, col, gt, body, mut lin, in_funnel) in &mut props {
        if *body != RigidBody::Dynamic || Some(pe) == held {
            continue;
        }
        let tr = gt.compute_transform();
        let center = tr.translation + tr.rotation * ((col.shape.mins + col.shape.maxs) * 0.5);
        match funnels.iter().find_map(|(_, f, _)| pull_at(&f, center)) {
            Some(pull) => {
                lin.0 = pull.dir * pull.speed + (pull.axis_point - center) * CENTERING;
                if !in_funnel {
                    commands.entity(pe).insert((InFunnel, GravityScale(0.0)));
                }
            }
            None if in_funnel => {
                commands.entity(pe).remove::<InFunnel>().insert(GravityScale(1.0));
            }
            None => {}
        }
    }
}

/// One translucent tube per funnel segment, tinted by direction.
fn draw_funnels(
    mut commands: Commands,
    mut funnels: Query<&mut Funnel>,
    mut transforms: Query<&mut Transform>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<EffectMaterial>>,
    mut shared: Local<Option<(Handle<Mesh>, Handle<EffectMaterial>, Handle<EffectMaterial>)>>,
    mut mats: Query<&mut MeshMaterial3d<EffectMaterial>>,
) {
    let (mesh, forward, reverse) = shared
        .get_or_insert_with(|| {
            (
                meshes.add(tube_mesh(24)),
                materials.add(EffectMaterial::translucent(Vec3::new(0.3, 0.6, 1.0) * 1.5, 0.25, 0.6, 0.01)),
                materials.add(EffectMaterial::translucent(Vec3::new(1.0, 0.55, 0.2) * 1.5, 0.25, -0.6, 0.01)),
            )
        })
        .clone();
    for mut f in &mut funnels {
        while f.tubes.len() < f.segments.len() {
            let e = commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(forward.clone()), Transform::default(), MapObject)).id();
            f.tubes.push(e);
        }
        while f.tubes.len() > f.segments.len() {
            if let Some(e) = f.tubes.pop() {
                commands.entity(e).despawn();
            }
        }
        let material = if f.force < 0.0 { reverse.clone() } else { forward.clone() };
        for (seg, &tube) in f.segments.iter().zip(&f.tubes) {
            if let Ok(mut t) = transforms.get_mut(tube) {
                *t = segment_transform(seg.start, seg.end, f.radius, f.radius);
            }
            if let Ok(mut m) = mats.get_mut(tube)
                && m.0 != material
            {
                m.0 = material.clone();
            }
        }
    }
}

impl crate::save::Restore for Funnel {
    fn keep(&mut self, live: Self) {
        self.segments = live.segments;
        self.tubes = live.tubes;
    }
}
