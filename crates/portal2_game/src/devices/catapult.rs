//! Faith plates (`trigger_catapult`): launch whatever steps onto them toward a
//! target entity, or along a fixed direction (`docs/re/gameplay.md`).
//!
//! Toward a target the launch takes `t = |target - origin| / speed` seconds
//! (the straight-line distance over `playerSpeed` or `physicsSpeed`) and the
//! velocity is `(target - origin) / t` plus `g t / 2` upward; players aim
//! their origin 32 units below the target so their middle passes through it.
//! With `useExactVelocity` the launch keeps exactly that speed and picks one
//! of the two ballistic arcs (`exactVelocityChoiceType`: 1 the first, 2 the
//! second, 0 the one closer to how the object is already moving), falling
//! back to the plain launch when the target is out of reach. With
//! `useThresholdCheck` only objects already moving within
//! `lowerThreshold`/`upperThreshold` of the launch speed (and, toward a
//! target, heading within `EntryAngleTolerance` of it) are launched, and
//! `onlyVelocityCheck` then just fires `OnCatapulted`. Launched players lose
//! air control for `AirCtrlSupressionTime` (0.25 s when not positive).
//! Launched objects fly without drag (the game sets their drag and damping
//! to zero, server.dll `sub_103F54A0` / `sub_103F5730`), so they land where
//! they are aimed and chains of plates carry them on; here the drag comes
//! back when they next hit something.

use avian3d::prelude::{AngularDamping, AngularVelocity, CollisionStart, LinearDamping, LinearVelocity, RigidBody};
use bevy::prelude::*;

use crate::entities::SpawnHandlers;
use crate::entities::io::{FireOutput, InputCall, InputHandlers};
use crate::entities::props::PhysicsProp;
use crate::player::Player;
use crate::player::movement::GRAVITY;
use portal2_core::coords::angles_forward;
use portal2_core::entity::{EntityRegistry, MapEntity};
use portal2_core::sets::PlayerMoveSet;
use portal2_world::collision::{Collider as TraceCollider, Physics};

pub struct CatapultPlugin;

impl Plugin for CatapultPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AirControl>().add_systems(FixedUpdate, (run_catapults.after(PlayerMoveSet), land_flung));
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("trigger_catapult", spawn_catapult);
        world.resource_mut::<InputHandlers>().register("trigger_catapult", catapult_input);
    }
}

/// While suppressed, airborne player input is ignored so launch arcs stay
/// true. Landing always restores control.
#[derive(Resource, Default)]
pub struct AirControl {
    pub suppressed_until: f64,
}

/// Air control suppression when `AirCtrlSupressionTime` is not positive.
const DEFAULT_AIR_SUPPRESS: f32 = 0.25;
/// Players aim this far below the target.
const PLAYER_TARGET_DROP: f32 = 32.0;
/// After a rejected entry the same object is ignored this long.
const REFIRE: f64 = 0.1;
/// A launched object's contacts this soon after the launch (the plate it
/// leaves) do not end its flight.
const FLIGHT_GRACE: f64 = 0.1;

/// An object a plate launched, flying without drag since `since`.
#[derive(Component)]
pub struct Flung {
    since: f64,
}

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct Catapult {
    pub enabled: bool,
    pub player_speed: f32,
    pub physics_speed: f32,
    pub target: Option<String>,
    pub direction: Vec3,
    pub use_exact: bool,
    pub exact_choice: i32,
    pub use_threshold: bool,
    pub lower_threshold: f32,
    pub upper_threshold: f32,
    pub entry_tolerance: f32,
    pub only_velocity_check: bool,
    pub air_suppress: f32,
    pub direction_suppresses_air: bool,
    pub angular_impulse: bool,
    pub launch_players: bool,
    pub launch_props: bool,
    #[serde(skip)]
    touching: Vec<Entity>,
    #[serde(skip)]
    refire: Vec<(Entity, f64)>,
}

/// Trigger spawnflags: clients, physics objects, everything.
const SF_CLIENTS: u32 = 1;
const SF_PHYSICS: u32 = 8;
const SF_EVERYTHING: u32 = 64;

fn spawn_catapult(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    world.entity_mut(e).insert(Catapult {
        enabled: !me.get_bool("startdisabled"),
        player_speed: me.get_f32("playerspeed").unwrap_or(450.0),
        physics_speed: me.get_f32("physicsspeed").unwrap_or(450.0),
        target: me.get("launchtarget").filter(|s| !s.is_empty()).map(|s| s.to_ascii_lowercase()),
        direction: me.get_vec3("launchdirection").unwrap_or(Vec3::ZERO),
        use_exact: me.get_bool("useexactvelocity"),
        exact_choice: me.get_i32("exactvelocitychoicetype").unwrap_or(0),
        use_threshold: me.get_bool("usethresholdcheck"),
        lower_threshold: me.get_f32("lowerthreshold").unwrap_or(0.15),
        upper_threshold: me.get_f32("upperthreshold").unwrap_or(0.30),
        entry_tolerance: me.get_f32("entryangletolerance").unwrap_or(0.0),
        only_velocity_check: me.get_bool("onlyvelocitycheck"),
        air_suppress: me.get_f32("airctrlsupressiontime").unwrap_or(-1.0),
        direction_suppresses_air: me.get_bool("directionsuppressaircontrol"),
        angular_impulse: me.get_bool("applyangularimpulse"),
        launch_players: me.spawnflags() & (SF_CLIENTS | SF_EVERYTHING) != 0,
        launch_props: me.spawnflags() & (SF_PHYSICS | SF_EVERYTHING) != 0,
        touching: Vec::new(),
        refire: Vec::new(),
    });
}

fn catapult_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut c) = world.get_mut::<Catapult>(call.target) else { return false };
    let v = source_formats::kv::parse_f32_lenient(&call.param);
    match call.input.as_str() {
        "enable" => c.enabled = true,
        "disable" => c.enabled = false,
        "toggle" => c.enabled = !c.enabled,
        "setplayerspeed" => c.player_speed = v,
        "setphysicsspeed" => c.physics_speed = v,
        "setlaunchtarget" => {
            let t = call.param.trim().to_ascii_lowercase();
            c.target = (!t.is_empty()).then_some(t);
        }
        "setexactvelocitychoicetype" => c.exact_choice = v as i32,
        _ => return false,
    }
    true
}

/// The plain launch: a flight time from the straight-line distance, then
/// enough lift to make up for gravity over it.
pub fn launch_velocity(from: Vec3, target: Vec3, speed: f32) -> Vec3 {
    let d = target - from;
    let t = d.length() / speed.max(1e-3);
    if t <= 1e-6 {
        return Vec3::Z * speed;
    }
    d / t + Vec3::Z * (GRAVITY * t * 0.5)
}

/// The two ballistic arcs of exactly `speed` through `target`, first the
/// steep one, or None when the target is out of reach.
pub fn exact_arcs(from: Vec3, target: Vec3, speed: f32) -> Option<[Vec3; 2]> {
    let d = target - from;
    let flat = d.with_z(0.0);
    let x = flat.length();
    if x == 0.0 {
        return None;
    }
    let s2 = speed * speed;
    let disc = s2 * s2 - GRAVITY * (GRAVITY * x * x + 2.0 * d.z * s2);
    if disc <= 0.0 {
        return None;
    }
    let dir = flat / x;
    let arc = |theta: f32| dir * (speed * theta.cos()) + Vec3::Z * (speed * theta.sin());
    let root = disc.sqrt();
    Some([arc(((s2 + root) / (GRAVITY * x)).atan()), arc(((s2 - root) / (GRAVITY * x)).atan())])
}

/// What happens to an object entering the plate.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Entry {
    Launch(Vec3),
    /// `onlyVelocityCheck`: fire the output, leave the object alone.
    Pass,
    /// Wrong speed or angle; ignored for a moment.
    Reject,
    Skip,
}

impl Catapult {
    fn speed(&self, player: bool) -> f32 {
        if player { self.player_speed } else { self.physics_speed }
    }

    /// The launch toward a target, as the plate would apply it.
    fn target_velocity(&self, origin: Vec3, target: Vec3, player: bool, moving: Vec3) -> Vec3 {
        let aim = if player { target - Vec3::Z * PLAYER_TARGET_DROP } else { target };
        let speed = self.speed(player);
        if self.use_exact
            && let Some([steep, flat]) = exact_arcs(origin, aim, speed)
        {
            let along = moving.normalize_or_zero();
            return match self.exact_choice {
                1 => steep,
                2 => flat,
                _ => {
                    if along.dot(steep.normalize_or_zero()) > along.dot(flat.normalize_or_zero()) {
                        steep
                    } else {
                        flat
                    }
                }
            };
        }
        launch_velocity(origin, aim, speed)
    }

    fn entry(&self, origin: Vec3, velocity: Vec3, target: Option<Vec3>, player: bool) -> Entry {
        let speed = self.speed(player);
        match target {
            Some(t) => {
                let launch = self.target_velocity(origin, t, player, velocity);
                if !self.use_threshold {
                    return Entry::Launch(launch);
                }
                let entering = velocity.normalize_or_zero().dot((t - origin).normalize_or_zero());
                if entering < self.entry_tolerance {
                    return Entry::Reject;
                }
                let need = launch.length();
                let have = velocity.length();
                if have <= need - self.lower_threshold * need || have >= need + self.upper_threshold * need {
                    return Entry::Reject;
                }
                if self.only_velocity_check { Entry::Pass } else { Entry::Launch(launch) }
            }
            None => {
                let forward = angles_forward(self.direction);
                if self.use_threshold {
                    let along = velocity.dot(forward);
                    if speed - self.lower_threshold * speed > along || along > speed + self.upper_threshold * speed {
                        return Entry::Skip;
                    }
                }
                if self.only_velocity_check {
                    return Entry::Pass;
                }
                if player {
                    let mut v = forward * speed;
                    // Straight up gets half as much again.
                    if v.x.abs() <= 0.001 && v.y.abs() <= 0.001 {
                        v.z = speed * 1.5;
                    }
                    Entry::Launch(v)
                } else {
                    // Objects always go up at the full speed.
                    Entry::Launch(Vec3::new(forward.x * speed, forward.y * speed, speed))
                }
            }
        }
    }

    fn refiring(&self, e: Entity, now: f64) -> bool {
        self.refire.iter().any(|(r, until)| *r == e && *until > now)
    }
}

/// A random angular velocity, each axis within ±`max` degrees per second.
fn random_spin(max: f32, seed: u32) -> Vec3 {
    let r = |k: u32| {
        let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(k.wrapping_mul(97));
        x ^= x >> 15;
        x = x.wrapping_mul(0x2c1b_3c6d);
        x ^= x >> 12;
        (x & 0xffff) as f32 / 65535.0 * 2.0 - 1.0
    };
    Vec3::new(r(1), r(2), r(3)) * max.to_radians()
}

fn run_catapults(
    time: Res<Time>,
    physics: Physics,
    registry: Res<EntityRegistry>,
    transforms: Query<&GlobalTransform>,
    mut catapults: Query<(Entity, &mut Catapult)>,
    mut players: Query<(Entity, &mut Player)>,
    mut props: Query<(Entity, &TraceCollider, &GlobalTransform, &RigidBody, &mut LinearVelocity, &mut AngularVelocity), With<PhysicsProp>>,
    mut air: ResMut<AirControl>,
    mut out: MessageWriter<FireOutput>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs_f64();
    let player = players.single_mut().ok();
    let (pe, mut p) = match player {
        Some((pe, p)) => (Some(pe), Some(p)),
        None => (None, None),
    };
    for (ce, mut c) in &mut catapults {
        if !c.enabled {
            c.touching.clear();
            continue;
        }
        c.refire.retain(|(_, until)| *until > now);
        let target_pos =
            c.target.as_ref().and_then(|t| registry.first_named(t)).and_then(|t| transforms.get(t).ok()).map(|t| t.translation());
        let mut now_touching = Vec::new();

        if let (Some(pe), Some(p)) = (pe, p.as_deref_mut())
            && c.launch_players
            && !p.noclip
        {
            let (mins, maxs) = p.state.hull();
            if physics.touches(ce, p.state.origin, mins, maxs) {
                now_touching.push(pe);
                if !c.touching.contains(&pe) && !c.refiring(pe, now) {
                    match c.entry(p.state.origin, p.state.velocity, target_pos, true) {
                        Entry::Launch(v) => {
                            p.state.velocity = v;
                            p.state.on_ground = false;
                            p.state.ground_entity = None;
                            p.ground_ref = None;
                            // Launches at a target always take air control
                            // away for a moment; plain directions only when asked.
                            if target_pos.is_some() || c.direction_suppresses_air {
                                let t = if c.air_suppress > 0.0 { c.air_suppress } else { DEFAULT_AIR_SUPPRESS };
                                air.suppressed_until = now + t as f64;
                            }
                            out.write(FireOutput::new(ce, "OnCatapulted").with_activator(Some(pe)));
                            info!("catapult launched player at {:.0?}", v);
                        }
                        Entry::Pass => {
                            out.write(FireOutput::new(ce, "OnCatapulted").with_activator(Some(pe)));
                        }
                        Entry::Reject => c.refire.push((pe, now + REFIRE)),
                        Entry::Skip => {}
                    }
                }
            }
        }

        for (prop, col, gt, body, mut lin, mut ang) in &mut props {
            if !c.launch_props || *body != RigidBody::Dynamic {
                continue;
            }
            let t = gt.compute_transform();
            let (mins, maxs) = crate::physics::world_bounds(&col.shape, t.translation, t.rotation);
            let center = (mins + maxs) * 0.5;
            if !physics.touches(ce, center, mins - center, maxs - center) {
                continue;
            }
            now_touching.push(prop);
            if c.touching.contains(&prop) || c.refiring(prop, now) {
                continue;
            }
            // Objects are aimed from their origin.
            match c.entry(t.translation, lin.0, target_pos, false) {
                Entry::Launch(v) => {
                    lin.0 = v;
                    commands.entity(prop).insert((LinearDamping(0.0), AngularDamping(0.0), Flung { since: now }));
                    let seed = prop.to_bits() as u32 ^ (now * 1000.0) as u32;
                    ang.0 = match target_pos {
                        Some(_) if c.angular_impulse => random_spin(150.0, seed),
                        Some(_) => Vec3::ZERO,
                        None => random_spin(50.0, seed),
                    };
                    out.write(FireOutput::new(ce, "OnCatapulted").with_activator(Some(prop)));
                }
                Entry::Pass => {
                    out.write(FireOutput::new(ce, "OnCatapulted").with_activator(Some(prop)));
                }
                Entry::Reject => c.refire.push((prop, now + REFIRE)),
                Entry::Skip => {}
            }
        }
        c.touching = now_touching;
    }
}

/// A launched object that hits something (or is picked up) flies with drag
/// again.
fn land_flung(
    time: Res<Time>,
    mut hits: MessageReader<CollisionStart>,
    flung: Query<&Flung>,
    holding: Res<crate::physics::carry::Holding>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs_f64();
    let mut land = |e: Entity| {
        if flung.get(e).is_ok_and(|f| now - f.since > FLIGHT_GRACE) {
            commands.entity(e).remove::<Flung>().insert((
                LinearDamping(crate::physics::PROP_LINEAR_DAMPING),
                AngularDamping(crate::physics::PROP_ANGULAR_DAMPING),
            ));
        }
    };
    for hit in hits.read() {
        land(hit.body1.unwrap_or(hit.collider1));
        land(hit.body2.unwrap_or(hit.collider2));
    }
    if let Some(held) = holding.entity {
        land(held);
    }
}

impl portal2_core::save::Restore for Catapult {}

#[cfg(test)]
mod tests {
    use super::*;

    fn fly(from: Vec3, v: Vec3, t: f32) -> Vec3 {
        from + v * t + Vec3::NEG_Z * 0.5 * GRAVITY * t * t
    }

    #[test]
    fn launch_reaches_target_after_distance_over_speed() {
        let from = Vec3::ZERO;
        let target = Vec3::new(600.0, 200.0, 128.0);
        let v = launch_velocity(from, target, 450.0);
        let t = target.length() / 450.0;
        assert!(fly(from, v, t).distance(target) < 0.5);
    }

    #[test]
    fn exact_arcs_keep_speed_and_land() {
        let target = Vec3::new(400.0, 0.0, 64.0);
        let arcs = exact_arcs(Vec3::ZERO, target, 700.0).unwrap();
        assert!(arcs[0].z > arcs[1].z, "first arc is the steep one");
        for v in arcs {
            assert!((v.length() - 700.0).abs() < 0.1);
            let t = 400.0 / v.x;
            let z = v.z * t - 0.5 * GRAVITY * t * t;
            assert!((z - 64.0).abs() < 0.5, "{z}");
        }
        assert!(exact_arcs(Vec3::ZERO, Vec3::new(5000.0, 0.0, 0.0), 300.0).is_none());
    }

    #[test]
    fn players_aim_below_the_target_and_thresholds_filter() {
        let mut c = Catapult {
            enabled: true,
            player_speed: 600.0,
            physics_speed: 600.0,
            target: None,
            direction: Vec3::ZERO,
            use_exact: false,
            exact_choice: 0,
            use_threshold: false,
            lower_threshold: 0.15,
            upper_threshold: 0.3,
            entry_tolerance: 0.0,
            only_velocity_check: false,
            air_suppress: -1.0,
            direction_suppresses_air: false,
            angular_impulse: true,
            launch_players: true,
            launch_props: true,
            touching: Vec::new(),
            refire: Vec::new(),
        };
        let target = Vec3::new(500.0, 0.0, 132.0);
        let Entry::Launch(v) = c.entry(Vec3::ZERO, Vec3::ZERO, Some(target), true) else { panic!() };
        let aim = target - Vec3::Z * 32.0;
        assert!(fly(Vec3::ZERO, v, aim.length() / 600.0).distance(aim) < 0.5);
        c.use_threshold = true;
        assert_eq!(c.entry(Vec3::ZERO, Vec3::X * 50.0, Some(target), true), Entry::Reject);
        let need = v.length();
        assert!(matches!(c.entry(Vec3::ZERO, v.normalize() * need, Some(target), true), Entry::Launch(_)));
    }
}
