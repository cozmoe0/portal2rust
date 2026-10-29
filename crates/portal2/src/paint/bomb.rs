//! Paint bombs (`prop_paint_bomb`): physics balls of gel that burst when
//! they hit something. The burst traces the `paintbomb_*_angle_split`
//! directions out to `paintbomb_explosion_radius` and paints where they
//! land; a ring of short-lived streaking blobs shows the splash. The ball
//! itself is drawn as a cluster of blobs, as the game draws it
//! (`cl_draw_paint_bomb_with_blobs`), over its hidden editor model.
//! A bomb spawns inside its dropper's pipe (one solid hull); like objects
//! the game's physics finds already overlapping something, it passes
//! through until it is clear.

use avian3d::prelude::{CollisionLayers, LinearVelocity};
use bevy::prelude::*;

use super::blobs::{Blob, Blobs};
use super::{PaintMap, PaintType, SplatSettings};
use crate::audio::SoundRequest;
use crate::clock::ThinkSet;
use crate::collision::{Physics, mask};
use crate::console::Console;
use crate::entities::io::FireOutput;
use crate::entities::{MapEntity, SpawnHandlers};

const TICK: f32 = 1.0 / 60.0;
/// The ball's size for impact tests and drawing.
const BALL_RADIUS: f32 = 12.0;
const CLUSTER_BLOBS: usize = 20;
/// Slower bumps (resting in a dropper, rolling) do not burst the bomb.
const BURST_SPEED: f32 = 100.0;

pub struct PaintBombPlugin;

impl Plugin for PaintBombPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, burst_bombs.in_set(ThinkSet));
        app.world_mut().resource_mut::<SpawnHandlers>().register("prop_paint_bomb", spawn_bomb);
        app.world_mut().resource_mut::<crate::entities::io::InputHandlers>().register("prop_paint_bomb", bomb_input);
    }
}

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct PaintBomb {
    pub paint: PaintType,
    last_speed: f32,
    /// Still clearing what it spawned inside, colliding with nothing.
    #[serde(default)]
    passing: bool,
    /// Its collision layers while passing.
    #[serde(skip)]
    layers: Option<CollisionLayers>,
    /// Burst last tick; removed once its `OnExploded` has gone out.
    #[serde(default)]
    burst: bool,
}

impl crate::save::Restore for PaintBomb {}

pub fn register_save(r: &mut crate::save::SaveRegistry) {
    r.component::<PaintBomb>("paint_bomb");
}

fn spawn_bomb(world: &mut World, e: Entity) {
    crate::entities::props::spawn_physics_prop(world, e);
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let paint = PaintType::from_index(me.get_i32("painttype").unwrap_or(4)).unwrap_or(PaintType::Erase);
    let spawn_sound = me.get_i32("playspawnsound").unwrap_or(1) != 0;
    world.entity_mut(e).insert((PaintBomb { paint, last_speed: 0.0, passing: true, layers: None, burst: false }, Visibility::Hidden));
    if spawn_sound {
        world.write_message(SoundRequest::on("PaintBlob.Inception", e));
    }
}

/// Bombs funnel into floor portals like other props, when allowed.
fn bomb_input(world: &mut World, call: &crate::entities::io::InputCall) -> bool {
    if !matches!(call.input.as_str(), "enableportalfunnel" | "disableportalfunnel") {
        return false;
    }
    if let Some(mut me) = world.get_mut::<MapEntity>(call.target) {
        me.set("allowfunnel", if call.input == "enableportalfunnel" { "1" } else { "0" });
    }
    true
}

/// Unit directions of the burst: `h` around by `v` from straight down to
/// straight up.
fn burst_directions(h: usize, v: usize) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(h * v);
    for j in 0..v {
        let pitch = -std::f32::consts::FRAC_PI_2 + (j as f32 + 0.5) * std::f32::consts::PI / v as f32;
        for i in 0..h {
            let yaw = i as f32 * std::f32::consts::TAU / h as f32;
            out.push(Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin()));
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn burst_bombs(
    mut commands: Commands,
    mut bombs: Query<(Entity, &mut PaintBomb, &GlobalTransform, Option<&LinearVelocity>, Option<&CollisionLayers>)>,
    physics: Physics,
    console: Res<Console>,
    map: Option<ResMut<PaintMap>>,
    mut blobs: ResMut<Blobs>,
    mut outputs: MessageWriter<FireOutput>,
    mut sounds: MessageWriter<SoundRequest>,
) {
    let Some(mut map) = map else { return };
    for (e, mut bomb, gt, velocity, layers) in &mut bombs {
        if bomb.burst {
            commands.entity(e).try_despawn();
            continue;
        }
        let pos = gt.translation();
        let v = velocity.map(|v| v.0).unwrap_or(Vec3::ZERO);
        let speed = v.length();
        if bomb.passing {
            let Some(layers) = layers else { continue };
            let r = Vec3::splat(BALL_RADIUS);
            let inside = physics.trace(pos, pos, -r, r, mask::SOLID_ALL, &[e]).start_solid;
            if inside {
                if bomb.layers.is_none() {
                    bomb.layers = Some(*layers);
                    commands.entity(e).insert(CollisionLayers::NONE);
                }
                continue;
            }
            if let Some(saved) = bomb.layers.take() {
                commands.entity(e).insert(saved);
            }
            bomb.passing = false;
            bomb.last_speed = speed;
            continue;
        }
        // About to hit something along its path, or just stopped hard.
        let ahead = v.normalize_or_zero() * (speed * TICK * 1.5 + BALL_RADIUS);
        let probe = physics.trace(pos, pos + ahead, Vec3::ZERO, Vec3::ZERO, mask::SOLID_ALL, &[e]);
        let hit = speed > BURST_SPEED && probe.hit();
        let stopped = bomb.last_speed > BURST_SPEED && bomb.last_speed - speed > BURST_SPEED;
        bomb.last_speed = speed;
        if !hit && !stopped {
            continue;
        }
        let settings = SplatSettings::from_console(&console);
        let reach = console.f32("paintbomb_explosion_radius");
        let radius = console.f32(if bomb.paint == PaintType::Erase { "sv_erase_surface_sphere_radius" } else { "sv_paint_surface_sphere_radius" });
        let coat = console.f32("sv_paint_alpha_coat");
        let dirs = burst_directions(console.i32("paintbomb_horizontal_angle_split").max(1) as usize, console.i32("paintbomb_vertical_angle_split").max(1) as usize);
        let (smin, smax) = (console.f32("paintbomb_streak_speed_min"), console.f32("paintbomb_streak_speed_max"));
        for (i, d) in dirs.iter().enumerate() {
            let tr = physics.trace(pos, pos + *d * reach, Vec3::ZERO, Vec3::ZERO, mask::SOLID_BRUSH_ONLY, &[e]);
            if tr.hit() && !tr.start_solid {
                map.paint_sphere(0, tr.end + tr.normal * 0.1, radius, coat, bomb.paint, &settings);
            }
            // A ring of splash blobs (they only draw).
            let t = (i as f32 * 0.618_034).fract();
            blobs.list.push(Blob::splash(pos, *d * (smin + (smax - smin) * t), bomb.paint, e));
        }
        debug!("paint bomb {e} burst at {pos} ({:?})", bomb.paint);
        sounds.write(SoundRequest::at("PaintBomb.Splat", pos));
        outputs.write(FireOutput { entity: e, output: "OnExploded".into(), activator: None, value: None });
        bomb.burst = true;
        commands.entity(e).insert(CollisionLayers::NONE);
    }
}

/// Positions of the cluster drawn for a bomb at time `t`, around its centre.
pub fn cluster_offsets(seed: u32, t: f32) -> [Vec3; CLUSTER_BLOBS] {
    let mut out = [Vec3::ZERO; CLUSTER_BLOBS];
    for (i, o) in out.iter_mut().enumerate() {
        let k = (seed.wrapping_mul(2654435761) ^ (i as u32).wrapping_mul(40503)) as f32;
        let (a, b, c) = ((k * 0.000_13).sin(), (k * 0.000_29).cos(), (k * 0.000_47).sin());
        let wobble = Vec3::new((t * 3.1 + a * 5.0).sin(), (t * 2.7 + b * 5.0).sin(), (t * 3.7 + c * 5.0).sin()) * 0.25;
        *o = (Vec3::new(a, b, c).normalize_or(Vec3::X) * (0.35 + 0.4 * ((k * 0.0007).sin().abs())) + wobble) * BALL_RADIUS;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_covers_the_sphere() {
        let d = burst_directions(8, 8);
        assert_eq!(d.len(), 64);
        assert!(d.iter().all(|v| (v.length() - 1.0).abs() < 1e-5));
        assert!(d.iter().any(|v| v.z < -0.9) && d.iter().any(|v| v.z > 0.9));
    }
}
