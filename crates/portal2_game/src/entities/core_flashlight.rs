//! Wheatley's flashlight: `EnableFlashlight` / `DisableFlashlight` on an
//! `npc_personality_core` (the dark stretches of `sp_a2_bts1` and
//! `sp_a2_bts3`).
//!
//! The server gives Wheatley's model, not the other cores', an
//! `env_projectedtexture` riding its `eyes` attachment: an 85° frustum with
//! shadows and the `effects/flashlight001_improved` cookie, the entity's
//! defaults otherwise (white, 4 to 750 units), turned off; the inputs send
//! it `TurnOn` and `TurnOff` (server.dll `sub_103C35E0`, `sub_103C3400`,
//! `sub_103C34F0`). Here it is made the first time it is switched on, so a
//! game saved before then still gets one. Like any projected texture,
//! turning it on turns the others off.
//!
//! While it is on, the client also draws the `flashlight_thirdperson`
//! particles from the eye along the beam: control point 1 on the eye, 2
//! where a trace 256 units ahead ends, 3 on the viewer (client.dll
//! `sub_103A0550`). They sit on the light entity, so they save with it.

use bevy::prelude::*;
use portal2_particles::ParticleEffect;

use super::io::{EventQueue, EventTarget};
use super::{MapEntity, map_parent, set_parent_attachment, spawn_runtime_entity};
use crate::visuals::projected::ProjectedTexture;
use portal2_core::view::MainCamera;
use portal2_render::models::StudioAnimator;

const BEAM: &str = "flashlight_thirdperson";
/// How far ahead of the eye the beam's particles reach at most.
const BEAM_REACH: f32 = 256.0;

pub fn register(app: &mut App) {
    app.add_systems(PostUpdate, aim_beams.after(TransformSystems::Propagate));
    app.add_systems(PostUpdate, dbg_cores.after(TransformSystems::Propagate));
}

/// `EnableFlashlight` (`on`) or `DisableFlashlight` on a core.
pub fn switch(world: &mut World, core: Entity, on: bool) {
    let light = match find_light(world, core) {
        Some(l) => Some(l),
        None if on => create_light(world, core),
        None => None,
    };
    info!(
        "DBGTMP flashlight switch core {core} at {:?} on {on} light {light:?} parent {:?} light at {:?} local {:?}",
        super::world_transform(world, core).translation(),
        light.and_then(|l| world.get::<ChildOf>(l).map(|c| c.parent())),
        light.map(|l| super::world_transform(world, l).translation()),
        light.and_then(|l| world.get::<Transform>(l).map(|t| t.translation))
    );
    if let Some(light) = light {
        let input = if on { "turnon" } else { "turnoff" };
        world.resource_mut::<EventQueue>().push(0.0, EventTarget::Entity(light), input, "", Some(core), Some(core));
    }
}

/// The core's flashlight, if it has one: the projected texture riding it
/// with the beam's particles.
fn find_light(world: &mut World, core: Entity) -> Option<Entity> {
    let lights: Vec<Entity> = world
        .query_filtered::<(Entity, &ParticleEffect), With<ProjectedTexture>>()
        .iter(world)
        .filter(|(_, fx)| fx.effect == BEAM)
        .map(|(e, _)| e)
        .collect();
    lights.into_iter().find(|&l| map_parent(world, l) == Some(core))
}

fn create_light(world: &mut World, core: Entity) -> Option<Entity> {
    // Only Wheatley's model carries one, not the other cores' (`altmodel`).
    if world.get::<MapEntity>(core).and_then(|m| m.get_i32("altmodel")).unwrap_or(0) != 0 {
        return None;
    }
    world.get::<StudioAnimator>(core)?.model.studio.mdl.find_attachment("eyes")?;
    let light = spawn_runtime_entity(
        world,
        "env_projectedtexture",
        &[("lightfov", "85.0"), ("enableshadows", "1"), ("texturename", "effects/flashlight001_improved"), ("spawnflags", "2")],
    );
    world.entity_mut(light).insert((ChildOf(core), ParticleEffect::new(BEAM, false)));
    set_parent_attachment(world, light, "eyes", false);
    Some(light)
}

/// Keeps each flashlight's beam particles running while it is on, along
/// where the eye looks.
fn aim_beams(
    physics: portal2_world::collision::Physics,
    camera: Query<&GlobalTransform, With<MainCamera>>,
    parents: Query<&ChildOf>,
    cores: Query<(), With<StudioAnimator>>,
    mut lights: Query<(&ProjectedTexture, &GlobalTransform, &mut ParticleEffect, &ChildOf)>,
    dbg_gt: Query<(&GlobalTransform, Option<&ChildOf>), Without<ProjectedTexture>>,
) {
    let viewer = camera.single().ok().map(|c| c.translation());
    for (light, at, mut fx, parent) in &mut lights {
        if fx.effect != BEAM {
            continue;
        }
        if light.on != fx.active {
            if light.on {
                fx.input("start");
            } else {
                fx.stop();
            }
        }
        if !light.on {
            continue;
        }
        // The trace ignores the core the light rides.
        let mut core = parent.parent();
        while !cores.contains(core) {
            let Ok(p) = parents.get(core) else { break };
            core = p.parent();
        }
        let eye = at.compute_transform();
        {
            let mut chain = vec![];
            let mut cur = parent.parent();
            for _ in 0..16 {
                let Ok((g, p)) = dbg_gt.get(cur) else { break };
                chain.push((cur, g.translation()));
                let Some(p) = p else { break };
                cur = p.parent();
            }
            info!("DBGTMP chain light {:.0?} {:?}", at.translation(), chain);
        }
        info!("DBGTMP flashlight at {:.0?} fwd {:.2?} cookie {} fov {}", eye.translation, eye.rotation * Vec3::X, light.cookie, light.fov);
        let ahead = eye.translation + eye.rotation * Vec3::X * BEAM_REACH;
        let mask = portal2_world::collision::mask::SOLID_ALL;
        let end = physics.trace(eye.translation, ahead, Vec3::ZERO, Vec3::ZERO, mask, &[core]).end;
        info!("DBGTMP beam end {:.0?}", end);
        fx.cp_frames = vec![(1, eye)];
        fx.cp_values = vec![(2, end), (3, viewer.unwrap_or(eye.translation))];
    }
}

fn dbg_cores(
    time: Res<Time>,
    cores: Query<(
        Entity,
        &MapEntity,
        &GlobalTransform,
        Option<&ChildOf>,
        Option<&avian3d::prelude::RigidBody>,
        Option<&avian3d::prelude::LinearVelocity>,
        Option<&avian3d::prelude::Position>,
        &Transform,
    )>,
    gts: Query<&GlobalTransform>,
) {
    if (time.elapsed_secs() % 1.0) >= time.delta_secs() {
        return;
    }
    for (e, me, gt, p, body, v, pos, t) in &cores {
        if me.classname == "npc_personality_core" {
            info!(
                "DBGTMP core {e} {body:?} v {v:?} pos {pos:?} local {:.0?} gt {:.0?} parent gt {:?}",
                t.translation,
                gt.translation(),
                p.and_then(|p| gts.get(p.parent()).ok()).map(|g| g.translation())
            );
        }
    }
}
