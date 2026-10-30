//! Lasers: emitters cast beams that pass through glass and portals, bounce
//! out of reflection cubes, run through relays, power catchers and targets,
//! burn the player and destroy turrets.

use bevy::prelude::*;

use super::{BeamSegment, EffectMaterial, linked_frames, segment_transform, trace_beam, tube_mesh, with_ancestors};
use crate::entities::SpawnHandlers;
use crate::entities::io::{FireOutput, InputCall, InputHandlers};
use crate::entities::spawner::{ModelOptions, attach_studio_model};
use crate::player::Player;
use crate::player::health::DamagePlayer;
use crate::portals::{Portal, Portals};
use portal2_core::entity::MapEntity;
use portal2_core::sets::PlayerMoveSet;
use portal2_render::models::StudioAnimator;
use portal2_world::MapObject;
use portal2_world::collision::{Collider as TraceCollider, Physics};
use source_formats::bsp::contents;

/// Lasers pass through glass and grates.
pub const LASER_MASK: u32 = contents::SOLID | contents::MOVEABLE | contents::MONSTER;
const BEAM_RADIUS: f32 = 1.5;
/// Beam damage per second while it touches the player.
const BURN_DPS: f32 = 60.0;
/// How strongly the beam pushes the player away (units/s added per hit).
const BURN_PUSH: f32 = 120.0;

pub struct LaserPlugin;

impl Plugin for LaserPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<LaserHitTurret>().add_systems(FixedUpdate, update_lasers.after(PlayerMoveSet)).add_systems(Update, draw_beams);
        let world = app.world_mut();
        {
            let mut s = world.resource_mut::<SpawnHandlers>();
            s.register("env_portal_laser", spawn_emitter);
            s.register("prop_laser_catcher", spawn_receiver);
            s.register("prop_laser_relay", spawn_receiver);
            s.register("point_laser_target", spawn_receiver);
        }
        world.resource_mut::<InputHandlers>().register("env_portal_laser", emitter_input);
    }
}

#[derive(Component, Default, serde::Serialize, serde::Deserialize)]
pub struct LaserEmitter {
    pub on: bool,
    /// Muzzle in the emitter's local space and the local beam direction.
    pub muzzle: Vec3,
    pub muzzle_dir: Vec3,
    pub lethal: bool,
    #[serde(skip)]
    pub segments: Vec<BeamSegment>,
    #[serde(skip)]
    beams: Vec<Entity>,
}

/// Catchers, relays and invisible laser targets.
#[derive(Component, Default, serde::Serialize, serde::Deserialize)]
pub struct LaserReceiver {
    pub powered: bool,
    hit: bool,
    /// Ticks since the beam last touched it (smooths flicker).
    unhit_ticks: u32,
}

/// A laser touched a turret this tick.
#[derive(Message, Clone, Copy, Debug)]
pub struct LaserHitTurret(pub Entity);

fn spawn_emitter(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let model = me.studio_model().and_then(|p| attach_studio_model(world, e, &p, ModelOptions::default()));
    // Prefer a model attachment named for the laser; fall back to the front.
    let (muzzle, muzzle_dir) = model
        .as_ref()
        .and_then(|m| {
            let mdl = &m.studio.mdl;
            let i = mdl
                .attachments
                .iter()
                .position(|a| a.name.to_ascii_lowercase().contains("laser"))
                .or_else(|| (!mdl.attachments.is_empty()).then_some(0))?;
            let a = &mdl.attachments[i];
            let bone = m.default_pose.get(a.bone.max(0) as usize).copied().unwrap_or(Mat4::IDENTITY);
            let world = bone * a.local;
            Some((world.transform_point3(Vec3::ZERO), world.transform_vector3(Vec3::X).normalize_or(Vec3::X)))
        })
        .unwrap_or((Vec3::X * 16.0, Vec3::X));
    let on = me.get_i32("startstate").unwrap_or(0) == 0;
    debug!("laser emitter {e}: on {on}, muzzle {muzzle:?} dir {muzzle_dir:?}");
    world.entity_mut(e).insert(LaserEmitter { on, muzzle, muzzle_dir, lethal: me.get_bool("lethaldamage"), ..default() });
}

fn spawn_receiver(world: &mut World, e: Entity) {
    if let Some(path) = world.get::<MapEntity>(e).and_then(|m| m.studio_model()) {
        attach_studio_model(world, e, &path, ModelOptions::default());
    }
    world.entity_mut(e).insert(LaserReceiver::default());
}

fn emitter_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut l) = world.get_mut::<LaserEmitter>(call.target) else { return false };
    match call.input.as_str() {
        "turnon" => l.on = true,
        "turnoff" => l.on = false,
        "toggle" => l.on = !l.on,
        _ => return false,
    }
    true
}

fn segment_point_distance(seg: &BeamSegment, p: Vec3) -> (f32, Vec3) {
    let d = seg.end - seg.start;
    let t = ((p - seg.start).dot(d) / d.length_squared().max(1e-6)).clamp(0.0, 1.0);
    let closest = seg.start + d * t;
    (closest.distance(p), closest)
}

fn update_lasers(
    time: Res<Time<Fixed>>,
    physics: Physics,
    portals: Res<Portals>,
    portal_query: Query<&Portal>,
    mut emitters: Query<(Entity, &mut LaserEmitter, &GlobalTransform)>,
    mut receivers: Query<(Entity, &mut LaserReceiver, &GlobalTransform, &MapEntity, Option<&mut StudioAnimator>)>,
    cubes: Query<(&MapEntity, &GlobalTransform, &TraceCollider)>,
    players: Query<(Entity, &Player)>,
    parents: Query<&ChildOf>,
    mut damage: MessageWriter<DamagePlayer>,
    mut turret_hits: MessageWriter<LaserHitTurret>,
    mut out: MessageWriter<FireOutput>,
) {
    let frames = linked_frames(&portals, &portal_query);
    let dt = time.delta_secs();
    for (_, mut r, _, _, _) in &mut receivers {
        r.hit = false;
    }
    let player = players.single().ok();
    for (e, mut laser, gt) in &mut emitters {
        if !laser.on {
            laser.segments.clear();
            continue;
        }
        let affine = gt.affine();
        let start = affine.transform_point3(laser.muzzle);
        let dir = affine.transform_vector3(laser.muzzle_dir);
        let redirect = |hit: Entity| -> Option<(Vec3, Vec3)> {
            let (me, t, col) = cubes.get(hit).ok()?;
            match me.classname.as_str() {
                // Reflection cubes re-emit the beam along their facing.
                "prop_weighted_cube" if me.get_i32("cubetype") == Some(2) => {
                    let tr = t.compute_transform();
                    let center = tr.translation + tr.rotation * ((col.shape.mins + col.shape.maxs) * 0.5);
                    Some((center, tr.rotation * Vec3::X))
                }
                // Relays let the beam run on through them.
                "prop_laser_relay" => {
                    let tr = t.compute_transform();
                    Some((tr.translation + dir * 24.0, dir))
                }
                _ => None,
            }
        };
        let ignore = with_ancestors(e, &parents);
        let segments = trace_beam(&physics, frames, start, dir, LASER_MASK, &ignore, 16, redirect);
        // Receivers hit by the beam's ends, and targets along its path.
        for seg in &segments {
            if let Some(hit) = seg.hit_entity
                && let Ok((_, mut r, _, me, _)) = receivers.get_mut(hit)
                && me.classname != "point_laser_target"
            {
                r.hit = true;
            }
            if let Some(hit) = seg.hit_entity
                && cubes.get(hit).is_ok_and(|(m, _, _)| m.classname == "npc_portal_turret_floor")
            {
                turret_hits.write(LaserHitTurret(hit));
            }
            for (_, mut r, rt, me, _) in &mut receivers {
                if me.classname == "point_laser_target" && segment_point_distance(seg, rt.translation()).0 < 12.0 {
                    r.hit = true;
                }
            }
            if let Some((_, p)) = player
                && !p.noclip
            {
                let (mins, maxs) = p.state.hull();
                let center = p.state.origin + (mins + maxs) * 0.5;
                // Distance from the beam to the player's vertical axis.
                let (dist, closest) = segment_point_distance(seg, center);
                if dist < 20.0 && (closest.z - center.z).abs() < (maxs.z - mins.z) * 0.5 {
                    let away = (center - closest).with_z(0.0).normalize_or(-seg.dir.with_z(0.0).normalize_or(Vec3::X));
                    let amount = if laser.lethal { 1000.0 } else { BURN_DPS * dt };
                    damage.write(DamagePlayer { amount, push: away * BURN_PUSH * dt * 10.0, source: Some(e) });
                }
            }
        }
        let moved =
            segments.len() != laser.segments.len() || segments.iter().zip(&laser.segments).any(|(a, b)| a.end.distance(b.end) > 1.0);
        if moved {
            let first = physics.trace(start, start + dir * 4.0, Vec3::ZERO, Vec3::ZERO, LASER_MASK, &ignore);
            debug!(
                "laser {e} from {:?} dir {:?} (start solid {} contents {:#x} entity {:?} surface {}) path: {:?}",
                start.round(),
                dir,
                first.start_solid,
                first.contents,
                first.entity.and_then(|h| {
                    cubes
                        .get(h)
                        .ok()
                        .map(|(m, t, _)| (h, m.classname.clone(), t.translation().round(), parents.get(h).ok().map(|p| p.parent())))
                }),
                first.surface,
                segments.iter().map(|s| (s.end.round(), s.hit_entity, s.through_portal)).collect::<Vec<_>>()
            );
        }
        laser.segments = segments;
    }
    for (re, mut r, _, me, anim) in &mut receivers {
        if r.hit {
            r.unhit_ticks = 0;
        } else {
            r.unhit_ticks += 1;
        }
        let powered = r.hit || (r.powered && r.unhit_ticks < 3);
        if powered != r.powered {
            r.powered = powered;
            out.write(FireOutput::new(re, if powered { "OnPowered" } else { "OnUnpowered" }));
            if let Some(mut a) = anim
                && me.classname != "point_laser_target"
            {
                a.set_sequence(if powered { "spin" } else { "idle" });
            }
        }
    }
}

/// Keeps one glowing tube per beam segment.
fn draw_beams(
    mut commands: Commands,
    mut emitters: Query<&mut LaserEmitter>,
    mut transforms: Query<&mut Transform>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<EffectMaterial>>,
    mut shared: Local<Option<(Handle<Mesh>, Handle<EffectMaterial>)>>,
) {
    let (mesh, material) = shared
        .get_or_insert_with(|| (meshes.add(tube_mesh(8)), materials.add(EffectMaterial::glow(Vec3::new(1.0, 0.08, 0.04) * 4.0, 2.0, 0.01))))
        .clone();
    for mut laser in &mut emitters {
        while laser.beams.len() < laser.segments.len() {
            let e = commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), Transform::default(), MapObject)).id();
            laser.beams.push(e);
        }
        while laser.beams.len() > laser.segments.len() {
            if let Some(e) = laser.beams.pop() {
                commands.entity(e).despawn();
            }
        }
        for (seg, &beam) in laser.segments.iter().zip(&laser.beams) {
            if let Ok(mut t) = transforms.get_mut(beam) {
                *t = segment_transform(seg.start, seg.end, BEAM_RADIUS, BEAM_RADIUS);
            }
        }
    }
}

impl portal2_core::save::Restore for LaserEmitter {
    fn keep(&mut self, live: Self) {
        self.segments = live.segments;
        self.beams = live.beams;
    }
}

impl portal2_core::save::Restore for LaserReceiver {}
