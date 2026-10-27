//! Floor turrets: physics props that watch a cone in front of them, wind up
//! when the player is in sight, then fire until sight is lost. Tipping one
//! over, carrying it, hitting it with a laser or the map disabling it stops
//! it for good (or until re-enabled).

use avian3d::prelude::RigidBody;
use bevy::prelude::*;
use source_formats::bsp::contents;

use super::EffectMaterial;
use super::laser::LaserHitTurret;
use crate::collision::{Physics, mask};
use crate::entities::io::{FireOutput, InputCall, InputHandlers};
use crate::entities::props::PhysicsProp;
use crate::entities::spawner::{ModelOptions, attach_studio_model};
use crate::entities::{MapEntity, SpawnHandlers};
use crate::player::health::DamagePlayer;
use crate::player::{Player, PlayerMoveSet};
use crate::world::MapObject;

/// Half-angle of the view cone (cosine).
const VIEW_COS: f32 = 0.5;
const WIND_UP: f64 = 0.6;
/// Keep firing this long after losing sight.
const SIGHT_GRACE: f64 = 1.5;
const DAMAGE_PER_SECOND: f32 = 55.0;
/// Seconds on its side before a turret gives up.
const TIPPED_TIME: f64 = 1.5;
/// Walls, glass, props and light bridges block a turret's view.
const SIGHT_MASK: u32 = mask::SOLID_ALL | contents::BLOCKLOS;

pub struct TurretPlugin;

impl Plugin for TurretPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, run_turrets.after(PlayerMoveSet))
            .add_systems(Update, fade_tracers);
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("npc_portal_turret_floor", spawn_turret);
        world.resource_mut::<InputHandlers>().register("npc_portal_turret_floor", turret_input);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurretState {
    Searching,
    WindingUp,
    Firing,
    Dead,
}

#[derive(Component)]
pub struct Turret {
    pub enabled: bool,
    pub state: TurretState,
    pub range: f32,
    /// Eye position in the turret's local space.
    pub eye: Vec3,
    pub saw_player_at: f64,
    pub wind_started: f64,
    pub tipped_since: Option<f64>,
}

fn spawn_turret(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let opts = ModelOptions { skin: me.get_i32("skinnumber").unwrap_or(0).max(0) as usize, ..default() };
    let model = me.studio_model().and_then(|p| attach_studio_model(world, e, &p, opts));
    // Eye from an attachment whose name mentions it, else above the centre.
    let eye = model
        .as_ref()
        .and_then(|m| {
            let mdl = &m.studio.mdl;
            let a = mdl.attachments.iter().find(|a| a.name.to_ascii_lowercase().contains("eye"))?;
            let bone = m.default_pose.get(a.bone.max(0) as usize).copied().unwrap_or(Mat4::IDENTITY);
            Some((bone * a.local).transform_point3(Vec3::ZERO))
        })
        .or_else(|| model.as_ref().map(|m| (m.studio.mdl.hull_min + m.studio.mdl.hull_max) * 0.5 + Vec3::Z * 12.0))
        .unwrap_or(Vec3::Z * 40.0);
    world.entity_mut(e).insert((
        PhysicsProp,
        Turret {
            enabled: true,
            state: TurretState::Searching,
            range: me.get_f32("turretrange").unwrap_or(1024.0),
            eye,
            saw_player_at: f64::MIN,
            wind_started: 0.0,
            tipped_since: None,
        },
    ));
}

fn turret_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut t) = world.get_mut::<Turret>(call.target) else { return false };
    match call.input.as_str() {
        "enable" => t.enabled = true,
        "disable" => t.enabled = false,
        "toggle" => t.enabled = !t.enabled,
        "selfdestruct" | "selfdestructimmediately" => t.state = TurretState::Dead,
        "firebullet" | "enablegagging" | "disablegagging" | "enablepickup" | "disablepickup" => {}
        _ => return false,
    }
    true
}

/// Short-lived tracer line.
#[derive(Component)]
struct Tracer {
    until: f64,
}

#[allow(clippy::too_many_arguments)]
fn run_turrets(
    time: Res<Time>,
    physics: Physics,
    mut turrets: Query<(Entity, &mut Turret, &GlobalTransform, Option<&RigidBody>)>,
    players: Query<(Entity, &Player)>,
    holding: Option<Res<crate::physics::carry::Holding>>,
    mut lasered: MessageReader<LaserHitTurret>,
    mut damage: MessageWriter<DamagePlayer>,
    mut out: MessageWriter<FireOutput>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<EffectMaterial>>,
    mut shared: Local<Option<(Handle<Mesh>, Handle<EffectMaterial>)>>,
) {
    let now = time.elapsed_secs_f64();
    let dt = time.delta_secs();
    let held = holding.and_then(|h| h.entity);
    for hit in lasered.read() {
        if let Ok((e, mut t, _, _)) = turrets.get_mut(hit.0)
            && t.state != TurretState::Dead
        {
            t.state = TurretState::Dead;
            out.write(FireOutput::new(e, "OnDeath"));
        }
    }
    let player = players.single().ok().filter(|(_, p)| !p.noclip);
    for (e, mut t, gt, _) in &mut turrets {
        if t.state == TurretState::Dead {
            continue;
        }
        let tr = gt.compute_transform();
        // Lying on its side for a while ends it.
        let upright = (tr.rotation * Vec3::Z).z > 0.7;
        if !upright {
            let since = *t.tipped_since.get_or_insert(now);
            if now - since > TIPPED_TIME {
                t.state = TurretState::Dead;
                out.write(FireOutput::new(e, "OnTipped"));
                continue;
            }
        } else {
            t.tipped_since = None;
        }
        if !t.enabled || held == Some(e) || !upright {
            t.state = TurretState::Searching;
            continue;
        }
        let eye = tr.transform_point(t.eye);
        let forward = tr.rotation * Vec3::X;
        let sees = player.is_some_and(|(pe, p)| {
            let (mins, maxs) = p.state.hull();
            let target = p.state.origin + (mins + maxs) * 0.5;
            let to = target - eye;
            let dist = to.length();
            if dist > t.range || dist < 1.0 || forward.dot(to / dist) < VIEW_COS {
                return false;
            }
            let los = physics.trace(eye, target, Vec3::ZERO, Vec3::ZERO, SIGHT_MASK, &[e, pe]);
            los.fraction >= 1.0
        });
        if sees {
            t.saw_player_at = now;
        }
        let in_sight = now - t.saw_player_at < SIGHT_GRACE;
        t.state = match t.state {
            TurretState::Searching if sees => {
                t.wind_started = now;
                out.write(FireOutput::new(e, "OnFoundTarget"));
                TurretState::WindingUp
            }
            TurretState::WindingUp if !in_sight => TurretState::Searching,
            TurretState::WindingUp if now - t.wind_started >= WIND_UP => TurretState::Firing,
            TurretState::Firing if !in_sight => {
                out.write(FireOutput::new(e, "OnLostTarget"));
                TurretState::Searching
            }
            s => s,
        };
        if t.state == TurretState::Firing
            && sees
            && let Some((_, p)) = player
        {
            let (mins, maxs) = p.state.hull();
            let target = p.state.origin + (mins + maxs) * 0.5;
            let push = (target - eye).with_z(0.0).normalize_or_zero() * 4.0;
            damage.write(DamagePlayer { amount: DAMAGE_PER_SECOND * dt, push });
            // A tracer every few ticks.
            if (now * 20.0) as i64 % 3 == 0 {
                let (mesh, material) = shared
                    .get_or_insert_with(|| {
                        (
                            meshes.add(super::tube_mesh(4)),
                            materials.add(EffectMaterial::glow(Vec3::new(1.0, 0.8, 0.4) * 3.0, 6.0, 0.02)),
                        )
                    })
                    .clone();
                let jitter = Vec3::new(
                    ((now * 37.0).sin() as f32) * 6.0,
                    ((now * 53.0).cos() as f32) * 6.0,
                    ((now * 71.0).sin() as f32) * 6.0,
                );
                commands.spawn((
                    Tracer { until: now + 0.05 },
                    MapObject,
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    super::segment_transform(eye + forward * 8.0, target + jitter, 0.4, 0.4),
                ));
            }
        }
    }
}

fn fade_tracers(time: Res<Time>, tracers: Query<(Entity, &Tracer)>, mut commands: Commands) {
    let now = time.elapsed_secs_f64();
    for (e, t) in &tracers {
        if now >= t.until {
            commands.entity(e).despawn();
        }
    }
}
