//! The finale's bombs: `prop_exploding_futbol`, a physics ball that bursts
//! when it hits something in flight (`ExplodeOnTouch`) or on `Explode`,
//! and `point_futbol_shooter`, which throws a new one at its target on
//! `ShootFutbol`, at `launchSpeed` on the lower arc that reaches it under
//! gravity (straight at it when out of reach). A burst plays the base
//! explosion sound, pushes physics objects near it, hurts the player close
//! by, fires `OnExplode` and removes the ball.

use avian3d::prelude::LinearVelocity;
use bevy::prelude::*;

use crate::audio::SoundRequest;
use crate::clock::ThinkSet;
use crate::collision::{Physics, mask};
use crate::entities::io::{FireOutput, InputCall, InputHandlers, resolve_targets};
use crate::entities::{EntityRegistry, MapEntity, SpawnHandlers};
use crate::player::Player;

const TICK: f32 = 1.0 / 60.0;
const MODEL: &str = "models/props/futbol.mdl";
const BALL_RADIUS: f32 = 14.0;
/// Rolling and settling bumps do not set it off.
const TOUCH_SPEED: f32 = 100.0;
/// Blast reach and damage when the keyvalues leave them at 0.
const DEFAULT_RADIUS: f32 = 128.0;
const DEFAULT_DAMAGE: f32 = 50.0;

pub struct FutbolPlugin;

impl Plugin for FutbolPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, burst_futbols.in_set(ThinkSet));
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("prop_exploding_futbol", spawn_futbol);
        let mut h = world.resource_mut::<InputHandlers>();
        h.register("prop_exploding_futbol", futbol_input);
        h.register("point_futbol_shooter", shooter_input);
    }
}

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct ExplodingFutbol {
    on_touch: bool,
    radius: f32,
    damage: f32,
    last_speed: f32,
    #[serde(default)]
    burst: bool,
}

impl crate::save::Restore for ExplodingFutbol {}

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct FutbolShooter {
    #[serde(with = "crate::save::ids::opt_entity")]
    target: Option<Entity>,
    speed: f32,
}

impl crate::save::Restore for FutbolShooter {}

pub fn register_save(r: &mut crate::save::SaveRegistry) {
    r.component::<ExplodingFutbol>("exploding_futbol");
    r.component::<FutbolShooter>("futbol_shooter");
}

fn spawn_futbol(world: &mut World, e: Entity) {
    crate::entities::props::spawn_physics_prop(world, e);
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let futbol = ExplodingFutbol {
        on_touch: me.get_i32("explodeontouch").unwrap_or(1) != 0,
        radius: me.get_f32("exploderadius").filter(|r| *r > 0.0).unwrap_or(DEFAULT_RADIUS),
        damage: me.get_f32("explodedamage").filter(|d| *d > 0.0).unwrap_or(DEFAULT_DAMAGE),
        last_speed: 0.0,
        burst: false,
    };
    world.entity_mut(e).insert(futbol);
}

fn futbol_input(world: &mut World, call: &InputCall) -> bool {
    if call.input != "explode" {
        return false;
    }
    if let Some(mut f) = world.get_mut::<ExplodingFutbol>(call.target) {
        // Bursts on the next tick, where the blast is worked out.
        f.on_touch = true;
        f.last_speed = f32::MAX;
    }
    true
}

/// Launch velocity reaching `to` from `from` at `speed` on the lower arc.
fn launch_velocity(from: Vec3, to: Vec3, speed: f32, gravity: f32) -> Vec3 {
    let d = to - from;
    let flat = d.truncate();
    let x = flat.length();
    let (v2, g) = (speed * speed, gravity.max(1e-3));
    let disc = v2 * v2 - g * (g * x * x + 2.0 * d.z * v2);
    if x < 1e-3 || disc < 0.0 {
        return d.normalize_or(Vec3::X) * speed;
    }
    let angle = ((v2 - disc.sqrt()) / (g * x)).atan();
    let dir = flat / x;
    Vec3::new(dir.x * angle.cos(), dir.y * angle.cos(), angle.sin()) * speed
}

fn shooter_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    if world.get::<FutbolShooter>(e).is_none() {
        let speed = world.get::<MapEntity>(e).and_then(|m| m.get_f32("launchspeed")).unwrap_or(100.0);
        world.entity_mut(e).insert(FutbolShooter { target: None, speed });
    }
    match call.input.as_str() {
        "settarget" => {
            let target = resolve_targets(world, &call.param, call.activator, call.caller).into_iter().next();
            if let Some(mut s) = world.get_mut::<FutbolShooter>(e) {
                s.target = target;
            }
        }
        "shootfutbol" => {
            let Some(from) = world.get::<GlobalTransform>(e).map(|g| g.translation()) else { return true };
            let (target, speed) = world.get::<FutbolShooter>(e).map(|s| (s.target, s.speed)).unwrap_or((None, 100.0));
            let player = world.resource::<EntityRegistry>().player;
            let to = target.and_then(|t| {
                if Some(t) == player {
                    world.get::<Player>(t).map(|p| p.state.origin + Vec3::Z * 36.0)
                } else {
                    world.get::<GlobalTransform>(t).map(|g| g.translation())
                }
            });
            let Some(to) = to else { return true };
            let gravity = world.resource::<crate::console::Console>().f32("sv_gravity");
            let velocity = launch_velocity(from, to, speed, gravity);
            let origin = format!("{} {} {}", from.x, from.y, from.z);
            let ball = crate::entities::spawn_runtime_entity(
                world,
                "prop_exploding_futbol",
                &[("model", MODEL), ("origin", &origin), ("explodeontouch", "1")],
            );
            world.entity_mut(ball).insert(LinearVelocity(velocity));
        }
        _ => return false,
    }
    true
}

#[allow(clippy::type_complexity)]
fn burst_futbols(
    mut commands: Commands,
    mut balls: Query<(Entity, &mut ExplodingFutbol, &GlobalTransform, Option<&LinearVelocity>)>,
    mut bodies: Query<(&GlobalTransform, &mut LinearVelocity), Without<ExplodingFutbol>>,
    physics: Physics,
    holding: Option<Res<crate::physics::carry::Holding>>,
    registry: Res<EntityRegistry>,
    players: Query<&Player>,
    mut outputs: MessageWriter<FireOutput>,
    mut sounds: MessageWriter<SoundRequest>,
    mut damage: MessageWriter<crate::player::health::DamagePlayer>,
) {
    let held = holding.and_then(|h| h.entity);
    for (e, mut ball, gt, velocity) in &mut balls {
        if ball.burst {
            commands.entity(e).try_despawn();
            continue;
        }
        let pos = gt.translation();
        let v = velocity.map(|v| v.0).unwrap_or(Vec3::ZERO);
        let speed = v.length();
        let forced = ball.last_speed == f32::MAX;
        let mut hit = false;
        if ball.on_touch && Some(e) != held && speed > TOUCH_SPEED {
            let ahead = v / speed * (speed * TICK * 1.5 + BALL_RADIUS);
            let tr = physics.trace(pos, pos + ahead, Vec3::ZERO, Vec3::ZERO, mask::SOLID_ALL, &[e]);
            hit = tr.hit() && !tr.start_solid;
        }
        let stopped = !forced && ball.on_touch && Some(e) != held && ball.last_speed > TOUCH_SPEED * 2.0 && ball.last_speed - speed > TOUCH_SPEED * 2.0;
        ball.last_speed = speed;
        if !(forced || hit || stopped) {
            continue;
        }
        // The blast: pushes bodies, hurts the player nearby.
        for (bt, mut bv) in &mut bodies {
            let d = bt.translation() - pos;
            let dist = d.length();
            if dist < ball.radius && dist > 1e-3 {
                bv.0 += d / dist * (1.0 - dist / ball.radius) * 400.0;
            }
        }
        if let Some(p) = registry.player.and_then(|p| players.get(p).ok()) {
            let d = p.state.origin + Vec3::Z * 36.0 - pos;
            let dist = d.length();
            if dist < ball.radius {
                let amount = ball.damage * (1.0 - dist / ball.radius);
                damage.write(crate::player::health::DamagePlayer { amount, push: d.normalize_or(Vec3::Z) * amount * 4.0, source: Some(e) });
            }
        }
        sounds.write(SoundRequest::at("BaseExplosionEffect.Sound", pos));
        outputs.write(FireOutput { entity: e, output: "OnExplode".into(), activator: None, value: None });
        ball.burst = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_arc_reaches_the_target() {
        let (from, to, g) = (Vec3::ZERO, Vec3::new(400.0, 0.0, 0.0), 600.0);
        let v = launch_velocity(from, to, 600.0, g);
        assert!((v.length() - 600.0).abs() < 1e-2);
        // Fly it: lands at x = 400 when back at z = 0.
        let t = 2.0 * v.z / g;
        assert!((v.x * t - 400.0).abs() < 1.0, "{v} {t}");
        // Out of reach: straight at it.
        let v = launch_velocity(from, Vec3::new(5000.0, 0.0, 0.0), 100.0, g);
        assert!((v.normalize() - Vec3::X).length() < 1e-5);
    }
}
