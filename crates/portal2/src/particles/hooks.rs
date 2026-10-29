//! Particle effects the game starts from code rather than from map
//! entities (`docs/re/particles.md`, "Effects started by code"):
//!
//! - Portal shots: `portal_projectile_stream` flies from the gun to where
//!   the shot lands, starting `cl_portal_projectile_delay_sp` (0.1 s) after
//!   the shot and taking at least 0.1 s, along `(1 - t) start + t ((1 - t)
//!   aim point + t end)` where the aim point is the start pushed along the
//!   aim by the shot's length. Control point 2 holds the portal colour.
//! - Portals: `portal_success` when one opens, `portal_close` when one
//!   closes, `portal_badsurface` where a shot hits a surface that takes no
//!   portals, oriented by the portal (or the surface normal), with the
//!   portal colour on control point 2.
//! - Laser emitters: `laser_start_glow` at the muzzle while on, and
//!   `reflector_start_glow` on reflection cubes while a beam hits them.
//! - Laser catchers and relays: `laser_relay_powered` at their
//!   `particle_emitter` attachment while powered.
//! - Emancipation fields with `UseScanline` set (absent means off):
//!   `cleanser_scanline`, its
//!   control points 0 to 2 at three corners of the field's bounds (top,
//!   the other top corner, below the first), restarted whenever it ends.

use bevy::prelude::*;

use super::{ParticleEffect, spawn_effect};
use crate::devices::laser::{LaserEmitter, LaserReceiver};
use crate::entities::MapEntity;
use crate::entities::triggers::{Trigger, TriggerKind};
use crate::models::StudioAnimator;
use crate::portals::PortalFx;
use crate::world::{CurrentMap, MapObject};

/// `cl_portal_projectile_delay_sp`.
const PROJECTILE_DELAY: f32 = 0.1;
/// Shortest flight of the projectile.
const PROJECTILE_MIN_TIME: f32 = 0.1;

/// Portal colours on effects (blue, orange), 0..255.
pub const PORTAL_COLORS: [Vec3; 2] = [Vec3::new(0.0, 60.0, 255.0), Vec3::new(233.0, 78.0, 2.0)];

/// The flying portal shot.
#[derive(Component)]
pub struct PortalBlast {
    start: Vec3,
    aim_point: Vec3,
    end: Vec3,
    starts: f32,
    ends: f32,
}

/// Effects an entity's code started, so they can be stopped again.
#[derive(Component, Default)]
pub struct CodeEffects {
    glow: Option<Entity>,
}

pub struct HooksPlugin;

impl Plugin for HooksPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (portal_effects, fly_blasts, laser_glows, reflector_glows, relay_effects, scanlines));
    }
}

fn orientation(forward: Vec3, up_hint: Vec3) -> Quat {
    let f = forward.normalize_or(Vec3::X);
    let left = up_hint.cross(f).try_normalize().unwrap_or_else(|| f.any_orthonormal_vector());
    let up = f.cross(left);
    Quat::from_mat3(&Mat3::from_cols(f, left, up)).normalize()
}

/// A one-shot effect with the portal colour on control point 2.
fn effect_at(commands: &mut Commands, name: &str, at: Vec3, rotation: Quat, color: Vec3) -> Entity {
    let e = spawn_effect(commands, name, Transform::from_translation(at).with_rotation(rotation), true);
    let mut fx = ParticleEffect::new(name, true);
    fx.one_shot = true;
    fx.cp_values.push((2, color));
    commands.entity(e).insert(fx);
    e
}

fn portal_effects(time: Res<Time>, mut events: MessageReader<PortalFx>, mut commands: Commands) {
    let now = time.elapsed_secs();
    for ev in events.read() {
        match *ev {
            PortalFx::Shot { slot, start, end, aim } => {
                let starts = now + PROJECTILE_DELAY;
                let ends = now.max(starts + PROJECTILE_MIN_TIME);
                let aim_point = start + aim.normalize_or(Vec3::X) * start.distance(end);
                let e = effect_at(&mut commands, "portal_projectile_stream", start, orientation(aim, Vec3::Z), PORTAL_COLORS[slot.min(1)]);
                commands.entity(e).insert(PortalBlast { start, aim_point, end, starts, ends });
            }
            PortalFx::Opened { slot, frame } => {
                effect_at(&mut commands, "portal_success", frame.center, frame.rotation(), PORTAL_COLORS[slot.min(1)]);
            }
            PortalFx::Closed { slot, frame } => {
                effect_at(&mut commands, "portal_close", frame.center, frame.rotation(), PORTAL_COLORS[slot.min(1)]);
            }
            PortalFx::BadSurface { slot, at, normal } => {
                effect_at(&mut commands, "portal_badsurface", at, orientation(normal, Vec3::Z), PORTAL_COLORS[slot.min(1)]);
            }
        }
    }
}

/// Moves portal shots along their path.
fn fly_blasts(time: Res<Time>, mut blasts: Query<(&PortalBlast, &mut Transform)>) {
    let now = time.elapsed_secs();
    for (b, mut t) in &mut blasts {
        let f = ((now - b.starts) / (b.ends - b.starts).max(1e-4)).clamp(0.0, 1.0);
        let p = b.start * (1.0 - f) + (b.aim_point * (1.0 - f) + b.end * f) * f;
        t.translation = p;
    }
}

/// Keeps `laser_start_glow` on laser emitters that are on.
fn laser_glows(
    mut commands: Commands,
    mut emitters: Query<(Entity, &LaserEmitter, Option<&mut CodeEffects>)>,
    mut effects: Query<&mut ParticleEffect>,
) {
    for (e, laser, state) in &mut emitters {
        let glow = state.as_ref().and_then(|s| s.glow).filter(|g| effects.get(*g).is_ok());
        match (laser.on, glow) {
            (true, None) => {
                let rot = orientation(laser.muzzle_dir, Vec3::Z);
                let fx = commands
                    .spawn((
                        Name::new("laser_start_glow"),
                        Transform::from_translation(laser.muzzle).with_rotation(rot),
                        Visibility::default(),
                        ParticleEffect::new("laser_start_glow", true),
                        MapObject,
                        ChildOf(e),
                    ))
                    .id();
                match state {
                    Some(mut s) => s.glow = Some(fx),
                    None => {
                        commands.entity(e).insert(CodeEffects { glow: Some(fx) });
                    }
                }
            }
            (false, Some(g)) => {
                if let Ok(mut fx) = effects.get_mut(g) {
                    fx.stop();
                    fx.one_shot = true;
                }
                if let Some(mut s) = state {
                    s.glow = None;
                }
            }
            _ => {}
        }
    }
}

/// Reflection cubes a beam hits send it on; while they do they carry
/// `reflector_start_glow`.
#[derive(Component, Default)]
pub struct ReflectorGlow(Option<Entity>);

fn reflector_glows(
    mut commands: Commands,
    emitters: Query<&LaserEmitter>,
    mut cubes: Query<(Entity, &MapEntity, Option<&mut ReflectorGlow>)>,
    mut effects: Query<&mut ParticleEffect>,
) {
    let lit: Vec<Entity> = emitters.iter().flat_map(|l| l.segments.iter().filter_map(|s| s.hit_entity)).collect();
    for (e, me, state) in &mut cubes {
        if me.classname != "prop_weighted_cube" || me.get_i32("cubetype") != Some(2) {
            continue;
        }
        let on = lit.contains(&e);
        let glow = state.as_ref().and_then(|s| s.0).filter(|g| effects.get(*g).is_ok());
        match (on, glow) {
            (true, None) => {
                let fx = commands
                    .spawn((
                        Name::new("reflector_start_glow"),
                        Transform::IDENTITY,
                        Visibility::default(),
                        ParticleEffect::new("reflector_start_glow", true),
                        MapObject,
                        ChildOf(e),
                    ))
                    .id();
                match state {
                    Some(mut s) => s.0 = Some(fx),
                    None => {
                        commands.entity(e).insert(ReflectorGlow(Some(fx)));
                    }
                }
            }
            (false, Some(g)) => {
                if let Ok(mut fx) = effects.get_mut(g) {
                    fx.stop();
                    fx.one_shot = true;
                }
                if let Some(mut s) = state {
                    s.0 = None;
                }
            }
            _ => {}
        }
    }
}

/// `laser_relay_powered` on powered catchers and relays, at their
/// `particle_emitter` attachment.
fn relay_effects(
    mut commands: Commands,
    mut receivers: Query<(Entity, &LaserReceiver, &MapEntity, Option<&StudioAnimator>, Option<&mut CodeEffects>)>,
    mut effects: Query<&mut ParticleEffect>,
) {
    for (e, r, me, anim, state) in &mut receivers {
        if me.classname == "point_laser_target" {
            continue;
        }
        let glow = state.as_ref().and_then(|s| s.glow).filter(|g| effects.get(*g).is_ok());
        match (r.powered, glow) {
            (true, None) => {
                // On the attachment's bone when the model has one.
                let (parent, local) = anim
                    .and_then(|a| {
                        let mdl = &a.model.studio.mdl;
                        let i = mdl.find_attachment("particle_emitter")?;
                        let att = &mdl.attachments[i];
                        let joint = *a.joints.get(att.bone.max(0) as usize)?;
                        Some((joint, Transform::from_matrix(att.local)))
                    })
                    .unwrap_or((e, Transform::IDENTITY));
                let fx = commands
                    .spawn((
                        Name::new("laser_relay_powered"),
                        local,
                        Visibility::default(),
                        ParticleEffect::new("laser_relay_powered", true),
                        MapObject,
                        ChildOf(parent),
                    ))
                    .id();
                match state {
                    Some(mut s) => s.glow = Some(fx),
                    None => {
                        commands.entity(e).insert(CodeEffects { glow: Some(fx) });
                    }
                }
            }
            (false, Some(g)) => {
                if let Ok(mut fx) = effects.get_mut(g) {
                    fx.stop();
                    fx.one_shot = true;
                }
                if let Some(mut s) = state {
                    s.glow = None;
                }
            }
            _ => {}
        }
    }
}

/// `cleanser_scanline` on enabled emancipation fields that use it.
fn scanlines(
    mut commands: Commands,
    map: Option<Res<CurrentMap>>,
    mut fields: Query<(Entity, &Trigger, &MapEntity, &GlobalTransform, Option<&mut CodeEffects>)>,
    mut effects: Query<&mut ParticleEffect>,
) {
    let Some(map) = map else { return };
    for (e, t, me, gt, state) in &mut fields {
        // Fields without the key have it off (the game never sets it), not
        // on as the entity definitions' default says.
        let scanline = me.keyvalues.iter().find(|(k, _)| k == "usescanline").is_some_and(|(_, v)| v.trim() != "0");
        if !matches!(t.kind, TriggerKind::Cleanser) || !scanline {
            continue;
        }
        let glow = state.as_ref().and_then(|s| s.glow).filter(|g| effects.get(*g).is_ok());
        match (t.enabled, glow) {
            (true, None) => {
                let Some(model) = me.brush_model().and_then(|i| map.bsp.models.get(i)) else { continue };
                let offset = gt.translation();
                let (lo, hi) = (model.mins + offset, model.maxs + offset);
                if hi.z - lo.z < 32.0 {
                    continue;
                }
                let mut fx = ParticleEffect::new("cleanser_scanline", true);
                fx.repeat = true;
                fx.cp_values = vec![
                    (1, Vec3::new(lo.x, lo.y, hi.z)),
                    (2, Vec3::new(hi.x, hi.y, lo.z)),
                    (4, Vec3::new(lo.x, lo.y, lo.z - 512.0)),
                    (5, Vec3::new(lo.x, lo.y, lo.z - 512.0)),
                ];
                let id = commands
                    .spawn((Name::new("cleanser_scanline"), Transform::from_translation(hi), Visibility::default(), fx, MapObject))
                    .id();
                match state {
                    Some(mut s) => s.glow = Some(id),
                    None => {
                        commands.entity(e).insert(CodeEffects { glow: Some(id) });
                    }
                }
            }
            (false, Some(g)) => {
                if let Ok(mut fx) = effects.get_mut(g) {
                    fx.repeat = false;
                    fx.stop();
                    fx.one_shot = true;
                }
                if let Some(mut s) = state {
                    s.glow = None;
                }
            }
            _ => {}
        }
    }
}
