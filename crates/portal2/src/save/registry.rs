//! What a save holds besides entity keyvalues and placement: the state of
//! components and resources, by name. Most types are saved with serde as
//! they are; fields only the running game needs (render entities, loaded
//! scene data, sound channels) are skipped and kept from what the spawn code
//! built (see [`Restore`]).

use crate::entities::io::{EventQueue, EventTarget};
use crate::entities::logic;
use bevy::prelude::*;
use portal2_core::entity::MapEntity;
use portal2_core::save::{SaveRegistry, decode, encode};
use portal2_render::models::StudioAnimator;
use portal2_render::models::animation::Layer;

pub fn register_all(r: &mut SaveRegistry) {
    use crate::devices::{bridge, catapult, funnel, laser, security_camera, turret};
    use crate::entities::{buttons, detectors, movers, props, scenes, templates, trains, transition, triggers};
    use crate::sound::{ambient, soundscape};

    r.component::<logic::LogicRelay>("logic_relay");
    r.component::<logic::LogicTimer>("logic_timer");
    r.component::<logic::MathCounter>("math_counter");
    r.component::<logic::LogicBranch>("logic_branch");
    r.component::<logic::LogicCompare>("logic_compare");
    r.component::<logic::BranchListener>("branch_listener");
    logic::register_save(r);
    r.component::<movers::Mover>("mover");
    r.component::<movers::Rotating>("rotating");
    r.component::<movers::Toggleable>("toggleable");
    props::register_save(r);
    buttons::register_save(r);
    r.component::<detectors::PortalDetector>("portal_detector");
    crate::entities::script_conditions::register_save(r);
    crate::entities::viewcontrol::register_save(r);
    r.component::<crate::entities::RidesAttachment>("rides_attachment");
    r.component::<triggers::Trigger>("trigger");
    r.component::<trains::Train>("train");
    r.component::<trains::PathTrack>("path_track");
    r.component::<templates::Template>("template");
    transition::register_save(r);
    scenes::register_save(r);
    r.component::<laser::LaserEmitter>("laser_emitter");
    r.component::<laser::LaserReceiver>("laser_receiver");
    r.component::<bridge::LightBridge>("light_bridge");
    r.component::<funnel::Funnel>("funnel");
    r.component::<catapult::Catapult>("catapult");
    r.component::<turret::Turret>("turret");
    security_camera::register_save(r);
    ambient::register_save(r);
    r.component::<soundscape::SoundscapeEnt>("soundscape");
    crate::visuals::fog::register_save(r);
    crate::visuals::projected::register_save(r);
    crate::visuals::sprites::register_save(r);
    crate::particles::register_save(r);
    crate::visuals::spotlights::register_save(r);
    crate::visuals::movies::register_save(r);
    crate::visuals::tonemap::register_save(r);
    crate::paint::blobs::register_save(r);
    crate::paint::bomb::register_save(r);
    crate::paint::props::register_save(r);
    crate::devices::func_tank::register_save(r);
    crate::devices::futbol::register_save(r);
    r.custom("animator", save_animator, load_animator);
    crate::physics::constraints::register_save(r);
    r.custom("physics", save_physics, load_physics);
    r.custom("script_scope", crate::script::save::save_scope, crate::script::save::load_scope);

    r.resource::<EventQueue>("event_queue");
    r.resource::<logic::GlobalStates>("global_states");
    r.resource::<detectors::SpeedMod>("speed_mod");
    r.resource::<detectors::PlayerFog>("player_fog");
    templates::register_save_resources(r);
    r.resource::<crate::portals::gun::PortalGun>("portal_gun");
    r.resource::<crate::screen::ScreenFade>("screen_fade");
    r.resource::<crate::script::api::GladosSpokenFlags>("glados_spoken_flags");
    r.custom_resource("portals", save_portals, load_portals);
    r.resource::<crate::physics::carry::Holding>("holding");
}

/// The player's portal pair (placed again a frame after the load, once the
/// map change has cleared the old ones).
#[derive(Resource, serde::Serialize, serde::Deserialize)]
pub struct PendingPortals(pub [Option<crate::portals::placement::PortalFrame>; 2]);

fn save_portals(world: &World) -> Option<Result<Vec<u8>, String>> {
    // Saved again before the restored portals were placed: keep them.
    if let Some(pending) = world.get_resource::<PendingPortals>() {
        return Some(encode(pending));
    }
    let portals = world.get_resource::<crate::portals::Portals>()?;
    let frames = portals.entities.map(|e| e.and_then(|e| world.get::<crate::portals::Portal>(e)).map(|p| p.frame));
    Some(encode(&PendingPortals(frames)))
}

fn load_portals(world: &mut World, bytes: &[u8]) -> Result<(), String> {
    let saved: PendingPortals = decode(bytes)?;
    world.insert_resource(saved);
    Ok(())
}

pub fn place_saved_portals(world: &mut World) {
    let Some(PendingPortals(frames)) = world.remove_resource::<PendingPortals>() else { return };
    for (slot, frame) in frames.into_iter().enumerate() {
        if let Some(frame) = frame {
            world.write_message(crate::portals::PlacePortal { slot, frame, settle: false });
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AnimatorSave {
    sequence: String,
    cycle: f32,
    rate: f32,
    looping: bool,
    finished: bool,
    default_sequence: Option<String>,
    layers: Vec<(String, f32, f32)>,
    pose: Vec<(String, f32)>,
}

fn sequence_name(anim: &StudioAnimator, index: usize) -> String {
    anim.model.studio.sequence(index).map(|(m, l)| m.mdl.sequences[l].label.clone()).unwrap_or_default()
}

fn save_animator(world: &World, e: Entity) -> Option<Result<Vec<u8>, String>> {
    let a = world.get::<StudioAnimator>(e)?;
    let studio = &a.model.studio;
    let save = AnimatorSave {
        sequence: sequence_name(a, a.sequence),
        cycle: a.cycle,
        rate: a.rate,
        looping: a.looping,
        finished: a.finished,
        default_sequence: a.default_sequence.map(|i| sequence_name(a, i)),
        layers: a.layers().iter().map(|l| (sequence_name(a, l.sequence), l.cycle, l.weight)).collect(),
        pose: studio
            .pose_parameters
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name.clone(), a.pose_parameter(&p.name).unwrap_or_else(|| studio.pose_value(i, 0.5))))
            .collect(),
    };
    Some(encode(&save))
}

fn load_animator(world: &mut World, e: Entity, bytes: &[u8]) -> Result<(), String> {
    let s: AnimatorSave = decode(bytes)?;
    let Some(mut a) = world.get_mut::<StudioAnimator>(e) else { return Ok(()) };
    let find = |a: &StudioAnimator, name: &str| a.model.studio.find_sequence(name);
    if let Some(i) = find(&a, &s.sequence) {
        a.set_sequence_index(i);
    }
    a.cycle = s.cycle;
    a.rate = s.rate;
    a.looping = s.looping;
    a.finished = s.finished;
    a.default_sequence = s.default_sequence.as_deref().and_then(|n| find(&a, n));
    let layers =
        s.layers.iter().filter_map(|(n, cycle, weight)| Some(Layer { sequence: find(&a, n)?, cycle: *cycle, weight: *weight })).collect();
    a.set_layers(layers);
    for (name, value) in &s.pose {
        a.set_pose_parameter(name, *value);
    }
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PhysicsSave {
    linear: Vec3,
    angular: Vec3,
}

fn save_physics(world: &World, e: Entity) -> Option<Result<Vec<u8>, String>> {
    use avian3d::prelude::{AngularVelocity, LinearVelocity};
    world.get::<crate::entities::props::PhysicsProp>(e)?;
    let linear = world.get::<LinearVelocity>(e).map_or(Vec3::ZERO, |v| v.0);
    let angular = world.get::<AngularVelocity>(e).map_or(Vec3::ZERO, |v| v.0);
    Some(encode(&PhysicsSave { linear, angular }))
}

fn load_physics(world: &mut World, e: Entity, bytes: &[u8]) -> Result<(), String> {
    use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
    let s: PhysicsSave = decode(bytes)?;
    // Bodies are not parented, so their transform is their world placement.
    let t = world.get::<Transform>(e).copied().unwrap_or_default();
    let mut em = world.entity_mut(e);
    em.insert((LinearVelocity(s.linear), AngularVelocity(s.angular), Position(t.translation), Rotation(t.rotation)));
    Ok(())
}

/// Work entities do again when a game is restored: `logic_auto` fires on
/// every load (its spawn scheduled that, but the saved event queue replaced
/// the spawn-time one).
pub fn after_restore(world: &mut World) {
    crate::entities::scenes::resume_speech(world);
    if let Some(held) = world.get_resource::<crate::physics::carry::Holding>().and_then(|h| h.entity) {
        crate::physics::carry::resume_hold(world, held);
    }
    let autos: Vec<Entity> = {
        let mut q = world.query::<(Entity, &MapEntity)>();
        q.iter(world).filter(|(_, m)| m.classname == "logic_auto").map(|(e, _)| e).collect()
    };
    for e in autos {
        let pending = world.resource::<EventQueue>().has_input_for(e, "__autothink");
        if !pending {
            world.resource_mut::<EventQueue>().push(0.2, EventTarget::Entity(e), "__autothink", "", None, None);
        }
    }
}
