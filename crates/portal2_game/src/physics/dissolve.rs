//! Fizzling and dissolving, as the game does both (server.dll and
//! client.dll, `docs/re/gameplay.md`).
//!
//! A fizzler field touching a prop, or a weighted cube's `Dissolve` input,
//! runs the game's fizzle (`CTriggerPortalCleanser::FizzleBaseAnimating`,
//! `sub_10397480`): the prop is replaced by a non-solid copy of its model
//! that floats without gravity, and the copy dissolves
//! (`CBaseAnimating::Dissolve`, an `env_entity_dissolver`). Here the prop's
//! own entity becomes that copy. A dissolving model darkens to black over
//! the first second, fades out from 1.9 to 2.0 s and is gone at 2.0 s
//! (`C_EntityDissolve::ClientThink`); the `dissolve` particle system rides
//! it from the start (`cl_portal_use_new_dissolve`).

use std::collections::HashMap;

use avian3d::prelude::*;
use bevy::prelude::*;

use super::carry::Holding;
use crate::entities::io::fire_output;
use crate::entities::props::PhysicsProp;
use portal2_core::clock::GameClock;
use portal2_core::entity::{EntityRegistry, MapEntity};
use portal2_render::source_material::ModelMaterial;
use portal2_world::collision::Collider as TraceCollider;

/// The dissolver's timings (`CEntityDissolve::Spawn`): the model darkens
/// from `FADE_IN_START` over `FADE_IN_LENGTH`, its alpha falls from
/// `FADE_OUT_MODEL_START` over `FADE_OUT_MODEL_LENGTH`, and target and
/// dissolver are removed at `FADE_OUT_START + FADE_OUT_LENGTH`.
const FADE_IN_START: f64 = 0.0;
const FADE_IN_LENGTH: f64 = 1.0;
const FADE_OUT_MODEL_START: f64 = 1.9;
const FADE_OUT_MODEL_LENGTH: f64 = 0.1;
const FADE_OUT_START: f64 = 2.0;
const FADE_OUT_LENGTH: f64 = 0.0;

/// The particle system a dissolving model wears.
const DISSOLVE_PARTICLES: &str = "dissolve";

/// Classes a fizzler never touches (`off_1078F91C`).
const NEVER_FIZZLE: &[&str] = &[
    "func_door",
    "func_door_rotating",
    "prop_door_rotating",
    "func_tracktrain",
    "env_ghostanimating",
    "physicsshadowclone",
    "prop_energy_ball",
    "weapon_portalgun",
    "weapon_paintgun",
];

pub struct DissolvePlugin;

impl Plugin for DissolvePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, run_dissolves).add_systems(PostUpdate, tint_dissolving);
    }
}

/// A dissolving model: no longer a prop the map can address or the player
/// can touch, gone `FADE_OUT_START + FADE_OUT_LENGTH` seconds after `since`.
#[derive(Component)]
pub struct Dissolving {
    pub since: f64,
}

/// Fizzles `e` (`FizzleBaseAnimating`), from a fizzler field (`cleanser`)
/// or a weighted cube's `Dissolve` input.
pub fn fizzle(world: &mut World, e: Entity, cleanser: Option<Entity>) {
    if world.get::<Dissolving>(e).is_some() || world.get::<PhysicsProp>(e).is_none() {
        return;
    }
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let (class, name) = (me.classname.clone(), me.targetname.clone());
    if NEVER_FIZZLE.contains(&class.as_str()) {
        return;
    }
    // A personality core only reports it; it stays.
    if class == "npc_personality_core" {
        fire_output(world, e, "OnFizzled", Some(e), None);
        return;
    }
    if let Some(c) = cleanser
        && name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case("box"))
    {
        fire_output(world, c, "OnDissolveBox", Some(e), None);
    }
    // The copy keeps the prop's velocity; a carried prop is dropped first
    // and the copy takes the player's velocity, 4 units along the view
    // across the ground (`EyeDirection2D`) and 32 down.
    let mut lin = world.get::<LinearVelocity>(e).map_or(Vec3::ZERO, |v| v.0);
    let ang = world.get::<AngularVelocity>(e).map_or(Vec3::ZERO, |v| v.0);
    if world.resource::<Holding>().entity == Some(e) {
        super::carry::force_drop(world);
        let velocity = world.query::<&crate::player::Player>().single(world).ok().map(|p| p.state.velocity);
        if let Some(v) = velocity {
            let forward = portal2_core::coords::angles_forward(world.resource::<crate::player::PlayerInput>().input.angles);
            let flat = Vec3::new(forward.x, forward.y, 0.0).normalize_or_zero();
            lin = v + flat * 4.0 - Vec3::Z * 32.0;
        }
    }
    // `OnFizzled()`: the prop's output, itself as activator and caller.
    fire_output(world, e, "OnFizzled", Some(e), None);
    // The original goes: the map can no longer address it and saves leave
    // it out.
    world.resource_mut::<EntityRegistry>().remove(e);
    // The copy: not solid, no gravity, half the velocity, and spinning 20°/s
    // faster about its own vertical axis (the game adds to the local
    // angular velocity).
    let rotation = world.get::<GlobalTransform>(e).map_or(Quat::IDENTITY, |gt| gt.compute_transform().rotation);
    let mut ent = world.entity_mut(e);
    ent.remove::<(PhysicsProp, portal2_core::save::ids::SaveId, crate::entities::hints::Watched)>();
    ent.insert((
        GravityScale(0.0),
        CollisionLayers::NONE,
        LinearVelocity(lin * 0.5),
        AngularVelocity(ang + rotation * (Vec3::Z * 20f32.to_radians())),
    ));
    if let Some(mut c) = ent.get_mut::<TraceCollider>() {
        c.solid = false;
    }
    dissolve(world, e);
    if let Some(c) = cleanser {
        fire_output(world, c, "OnDissolve", Some(e), None);
    }
}

/// Starts `e` dissolving (`CBaseAnimating::Dissolve`, normal type).
fn dissolve(world: &mut World, e: Entity) {
    let now = world.resource::<GameClock>().curtime;
    world.entity_mut(e).insert(Dissolving { since: now });
    if let Some(at) = portal2_audio::sound_origin(world, e) {
        portal2_audio::emit(world, "Prop.Fizzled", portal2_audio::Emit { origin: Some(at), ..default() });
    }
    let at = world.get::<GlobalTransform>(e).map(|gt| gt.compute_transform()).unwrap_or_default();
    portal2_particles::spawn_effect_following(&mut world.commands(), DISSOLVE_PARTICLES, e, at);
    world.flush();
}

/// Removes dissolved models (`CEntityDissolve::DissolveThink`).
fn run_dissolves(clock: Res<GameClock>, dissolving: Query<(Entity, &Dissolving)>, mut commands: Commands) {
    for (e, d) in &dissolving {
        if clock.curtime - d.since >= FADE_OUT_START + FADE_OUT_LENGTH {
            commands.entity(e).despawn();
        }
    }
}

/// The dissolving model's colour at `t` seconds in: grey level (gamma
/// space) and alpha, as `C_EntityDissolve::ClientThink` sets its render
/// colour (bytes, render mode `kRenderTransColor`).
fn dissolve_color(t: f64) -> (f32, f32) {
    let fade_in = if t > FADE_OUT_START {
        1.0
    } else if t < FADE_IN_START {
        0.0
    } else if t > FADE_IN_START && t < FADE_IN_START + FADE_IN_LENGTH {
        (t - FADE_IN_START) / FADE_IN_LENGTH
    } else {
        1.0
    };
    let fade_out = if t > FADE_OUT_MODEL_START {
        let d = t - FADE_OUT_MODEL_START;
        if d <= FADE_OUT_MODEL_LENGTH { 1.0 - d / FADE_OUT_MODEL_LENGTH } else { 0.0 }
    } else {
        1.0
    };
    let byte = |v: f64| (v * 255.0) as u8 as f32 / 255.0;
    (byte(1.0 - fade_in), byte(fade_out))
}

/// Gives the meshes of dissolving models their own materials in the
/// dissolve colour. The models' shaders take the colour into linear space
/// (`SetModulationPixelShaderDynamicState_LinearColorSpace`, gamma 2.2);
/// alpha stays as is, and below 1 the model draws blended.
fn tint_dissolving(
    clock: Res<GameClock>,
    fixed: Res<Time<Fixed>>,
    dissolving: Query<(Entity, &Dissolving)>,
    children: Query<&Children>,
    mut meshes: Query<&mut MeshMaterial3d<ModelMaterial>>,
    mut materials: ResMut<Assets<ModelMaterial>>,
    mut own: Local<HashMap<Entity, (Handle<ModelMaterial>, Vec4, AlphaMode)>>,
) {
    own.retain(|mesh, _| meshes.contains(*mesh));
    let now = clock.curtime + fixed.overstep().as_secs_f64();
    for (root, d) in &dissolving {
        let (grey, alpha) = dissolve_color(now - d.since);
        let linear = grey.powf(2.2);
        for mesh in std::iter::once(root).chain(children.iter_descendants(root)) {
            let Ok(mut handle) = meshes.get_mut(mesh) else { continue };
            let entry = match own.get(&mesh) {
                Some(entry) => entry.clone(),
                None => {
                    let Some(m) = materials.get(&handle.0).cloned() else { continue };
                    let (color, mode) = (m.params.color, m.alpha_mode);
                    let entry = (materials.add(m), color, mode);
                    own.insert(mesh, entry.clone());
                    entry
                }
            };
            let (mine, color, mode) = entry;
            if handle.0 != mine {
                handle.0 = mine.clone();
            }
            let Some(mut m) = materials.get_mut(&mine) else { continue };
            m.params.color = color * Vec4::new(linear, linear, linear, alpha);
            m.alpha_mode = if alpha < 1.0 { AlphaMode::Blend } else { mode };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dissolve_darkens_then_fades_out() {
        assert_eq!(dissolve_color(-0.01), (1.0, 1.0));
        let (grey, _) = dissolve_color(0.001);
        assert!(grey > 0.99);
        let (grey, alpha) = dissolve_color(0.5);
        assert!((grey - 127.0 / 255.0).abs() < 1e-6 && alpha == 1.0);
        assert_eq!(dissolve_color(1.5), (0.0, 1.0));
        let (_, alpha) = dissolve_color(1.95);
        assert!((alpha - 127.0 / 255.0).abs() < 2.0 / 255.0);
        assert_eq!(dissolve_color(2.0), (0.0, 0.0));
    }
}
