//! Model-based entities: generic dynamic props, chamber doors, buttons,
//! weighted props and the map's static props.
//!
//! Model paths always come from data: the entity's `model` keyvalue or, when
//! the map leaves it out, the class default from the installed entity
//! definitions. Anything without a usable model is drawn as a plain box.

use bevy::ecs::system::SystemState;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use source_formats::bsp::StaticProp;
use source_formats::studio::StudioModel;
use source_formats::vhv::Vhv;

use super::io::{EventQueue, EventTarget, FireOutput, InputCall, InputHandlers, fire_output};
use super::spawner::{ModelOptions, attach_studio_model, clear_render_children};
use super::{MapEntity, SpawnHandlers};
use crate::world::{CurrentMap, MapObject};
use portal2_core::clock::GameClock;
use portal2_core::coords::angles_to_quat;
use portal2_core::sets::ThinkSet;
use portal2_core::vfs::GameFs;
use portal2_render::materials::{MaterialCache, MaterialContext};
use portal2_render::models::animation::{AnimationEvent, AnimationFinished};
use portal2_render::models::lighting;
use portal2_render::models::{ModelCache, SpawnModel, StudioAnimator, spawn_model_parts};
use portal2_render::source_material::{ModelMaterial, RefractMaterial};
use portal2_render::textures::TextureCache;

pub fn register(app: &mut App) {
    app.add_systems(Update, (animation_done_outputs, animation_events)).add_systems(FixedUpdate, button_timers.in_set(ThinkSet));
    let world = app.world_mut();
    {
        let mut s = world.resource_mut::<SpawnHandlers>();
        for class in ["prop_dynamic", "prop_dynamic_override", "prop_dynamic_ornament"] {
            s.register(class, spawn_dynamic);
        }
        s.register("prop_testchamber_door", spawn_chamber_door);
        s.register("npc_personality_core", spawn_personality_core);
        for class in ["prop_button", "prop_under_button"] {
            s.register(class, spawn_pedestal_button);
        }
        for class in ["prop_floor_button", "prop_floor_cube_button", "prop_floor_ball_button", "prop_under_floor_button"] {
            s.register(class, spawn_floor_button);
        }
        for class in ["prop_weighted_cube", "prop_physics", "prop_physics_override", "prop_physics_multiplayer"] {
            s.register(class, spawn_physics_prop);
        }
    }
    let mut h = world.resource_mut::<InputHandlers>();
    for class in ["prop_dynamic", "prop_dynamic_override", "prop_dynamic_ornament"] {
        h.register(class, dynamic_input);
    }
    h.register("prop_testchamber_door", chamber_door_input);
    for class in ["prop_weighted_cube", "prop_physics", "prop_physics_override", "prop_physics_multiplayer"] {
        h.register(class, physics_prop_input);
    }
    for class in ["prop_button", "prop_under_button"] {
        h.register(class, pedestal_button_input);
    }
    for class in ["prop_floor_button", "prop_floor_cube_button", "prop_floor_ball_button", "prop_under_floor_button"] {
        h.register(class, floor_button_input);
    }
    // Shared prop inputs (skin, animation) for every model entity.
    h.register_generic(model_inputs);
}

fn options_for(me: &MapEntity, animated: bool) -> ModelOptions {
    ModelOptions {
        skin: me.get_i32("skin").unwrap_or(0).max(0) as usize,
        body: me.get_i32("body").or_else(|| me.get_i32("setbodygroup")).unwrap_or(0),
        animated,
        light_origin: None,
    }
}

/// Attaches the entity's model (or a placeholder box) and returns whether a
/// real model was found.
fn attach_entity_model(world: &mut World, e: Entity, animated: bool) -> bool {
    if world.resource::<portal2_core::args::Args>().no_entity_models {
        return false;
    }
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return false };
    let path = me.studio_model();
    let attached = path.as_deref().and_then(|p| attach_studio_model(world, e, p, options_for(&me, animated))).is_some();
    if !attached {
        attach_placeholder(world, e);
    }
    attached
}

/// Plain grey box for entities whose model is missing.
fn attach_placeholder(world: &mut World, e: Entity) {
    let mut state: SystemState<(Commands, ResMut<Assets<Mesh>>, ResMut<Assets<ModelMaterial>>)> = SystemState::new(world);
    let (mut commands, mut meshes, mut mats) = state.get_mut(world).expect("render resources available");
    let mesh = meshes.add(Cuboid::new(16.0, 16.0, 16.0));
    let mat = mats.add(ModelMaterial::default());
    let child = commands.spawn((Mesh3d(mesh), MeshMaterial3d(mat), Transform::from_xyz(0.0, 0.0, 8.0))).id();
    commands.entity(e).add_child(child);
    state.apply(world);
}

/// The server's models for personality cores: Wheatley's, or with
/// `altmodel` set the one the other cores share; `ModelSkin` picks the skin.
const PERSONALITY_CORE_MODEL: &str = "models/npcs/personality_sphere/personality_sphere.mdl";
const PERSONALITY_CORE_ALT_MODEL: &str = "models/npcs/personality_sphere/personality_sphere_skins.mdl";

fn spawn_personality_core(world: &mut World, e: Entity) {
    if let Some(mut me) = world.get_mut::<MapEntity>(e) {
        if me.get("model").is_none_or(|m| !m.to_ascii_lowercase().ends_with(".mdl")) {
            let alt = me.get_i32("altmodel").unwrap_or(0) != 0;
            me.set("model", if alt { PERSONALITY_CORE_ALT_MODEL } else { PERSONALITY_CORE_MODEL });
        }
        if let Some(skin) = me.get("modelskin").map(str::to_owned) {
            me.set("skin", &skin);
        }
    }
    attach_entity_model(world, e, true);
    if let Some(mut anim) = world.get_mut::<StudioAnimator>(e) {
        let idle =
            anim.model.studio.find_sequence_by_activity("ACT_IDLE").or_else(|| anim.model.studio.find_sequence("sphere_idle_neutral"));
        if let Some(i) = idle {
            anim.set_sequence_index(i);
            anim.default_sequence = Some(i);
        }
    }
}

/// Default handling for `prop_*` classes without a dedicated spawner.
pub fn spawn_generic_prop(world: &mut World, e: Entity) {
    attach_entity_model(world, e, true);
}

/// `Dissolve`: the prop fizzles away (dropped first when carried).
fn physics_prop_input(world: &mut World, call: &InputCall) -> bool {
    if matches!(call.input.as_str(), "enableportalfunnel" | "disableportalfunnel") {
        if let Some(mut me) = world.get_mut::<MapEntity>(call.target) {
            me.set("allowfunnel", if call.input == "enableportalfunnel" { "1" } else { "0" });
        }
        return true;
    }
    if call.input != "dissolve" {
        return false;
    }
    let e = call.target;
    if let Some(mut holding) = world.get_resource_mut::<crate::physics::carry::Holding>()
        && holding.entity == Some(e)
    {
        holding.entity = None;
    }
    if let Some(at) = crate::audio::sound_origin(world, e) {
        crate::audio::emit(world, "Prop.Fizzled", crate::audio::Emit { origin: Some(at), ..default() });
    }
    fire_output(world, e, "OnFizzled", call.activator, None);
    super::despawn_entity(world, e);
    true
}

// ------------------------------------------------------------------ prop_dynamic

fn spawn_dynamic(world: &mut World, e: Entity) {
    attach_entity_model(world, e, true);
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    if let Some(mut anim) = world.get_mut::<StudioAnimator>(e) {
        if let Some(default) = me.get("defaultanim").filter(|s| !s.is_empty()) {
            anim.set_default_sequence(default);
            anim.set_sequence(default);
        }
        if let Some(rate) = me.get_f32("playbackrate").filter(|r| *r != 0.0) {
            anim.rate = rate;
        }
        if me.get_bool("randomanimation") {
            anim.cycle = 0.0;
        }
    }
    if me.get_bool("startdisabled")
        && let Some(mut v) = world.get_mut::<Visibility>(e)
    {
        *v = Visibility::Hidden;
    }
}

fn dynamic_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    match call.input.as_str() {
        "enable" | "turnon" => set_visible(world, e, true),
        "disable" | "turnoff" => set_visible(world, e, false),
        "enablecollision" | "disablecollision" | "becomeragdoll" | "fadeanddelete" => {}
        _ => return false,
    }
    true
}

fn set_visible(world: &mut World, e: Entity, visible: bool) {
    if let Some(mut v) = world.get_mut::<Visibility>(e) {
        *v = if visible { Visibility::Inherited } else { Visibility::Hidden };
    }
}

/// Inputs shared by every entity with a studio model.
fn model_inputs(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    match call.input.as_str() {
        "setanimation" | "setanimationnoreset" => {
            let name = call.param.trim().to_owned();
            let Some(mut anim) = world.get_mut::<StudioAnimator>(e) else { return false };
            if call.input == "setanimationnoreset" && anim.sequence_name().eq_ignore_ascii_case(&name) {
                return true;
            }
            if anim.set_sequence(&name) {
                fire_output(world, e, "OnAnimationBegun", call.activator, None);
            }
            true
        }
        "setdefaultanimation" => {
            let Some(mut anim) = world.get_mut::<StudioAnimator>(e) else { return false };
            anim.set_default_sequence(call.param.trim());
            if anim.finished || anim.looping {
                let name = call.param.trim().to_owned();
                anim.set_sequence(&name);
            }
            true
        }
        "setplaybackrate" => {
            let Some(mut anim) = world.get_mut::<StudioAnimator>(e) else { return false };
            anim.rate = source_formats::kv::parse_f32_lenient(&call.param);
            true
        }
        "skin" | "setbodygroup" => {
            if world.get::<StudioAnimator>(e).is_none() {
                return false;
            }
            let key = if call.input == "skin" { "skin" } else { "body" };
            let value = source_formats::kv::parse_i32_lenient(&call.param).to_string();
            let (sequence, cycle) = world.get::<StudioAnimator>(e).map(|a| (a.sequence, a.cycle)).unwrap_or((0, 0.0));
            if let Some(mut me) = world.get_mut::<MapEntity>(e) {
                me.set(key, &value);
            }
            clear_render_children(world, e);
            attach_entity_model(world, e, true);
            if let Some(mut anim) = world.get_mut::<StudioAnimator>(e) {
                anim.set_sequence_index(sequence);
                anim.cycle = cycle;
            }
            true
        }
        _ => false,
    }
}

fn animation_done_outputs(mut finished: MessageReader<AnimationFinished>, mut out: MessageWriter<FireOutput>) {
    for f in finished.read() {
        out.write(FireOutput::new(f.entity, "OnAnimationDone"));
    }
}

/// Model events the server and client act on: event 1100 sends the input
/// named in its options to the model's own entity (the container ride's
/// impacts are `FireUser1`-`4` on its master model), sound events play
/// their sound from the model, particle events start their effect at the
/// named attachment (or the model).
const SCRIPT_EVENT_FIRE_INPUT: i32 = 1100;

fn animation_events(mut events: MessageReader<AnimationEvent>, mut commands: Commands) {
    for ev in events.read() {
        let ev = ev.clone();
        commands.queue(move |world: &mut World| animation_event(world, &ev));
    }
}

fn animation_event(world: &mut World, ev: &AnimationEvent) {
    let e = ev.entity;
    if world.get_entity(e).is_err() {
        return;
    }
    if ev.event == SCRIPT_EVENT_FIRE_INPUT {
        let input = ev.options.trim().to_ascii_lowercase();
        world.resource_mut::<EventQueue>().push(0.0, EventTarget::Entity(e), &input, "", Some(e), Some(e));
        return;
    }
    match ev.name.as_str() {
        "AE_CL_PLAYSOUND" | "AE_SV_PLAYSOUND" => {
            crate::audio::emit(world, ev.options.trim(), crate::audio::Emit { entity: Some(e), ..default() });
        }
        "AE_CL_CREATE_PARTICLE_EFFECT" => {
            let mut parts = ev.options.split_whitespace();
            let Some(effect) = parts.next() else { return };
            let attachment = parts.nth(1);
            let at = attachment
                .and_then(|a| super::attachment_world(world, e, a))
                .unwrap_or_else(|| super::world_transform(world, e).compute_transform());
            let effect = effect.to_owned();
            let mut commands = world.commands();
            crate::particles::spawn_effect(&mut commands, &effect, at, true);
            world.flush();
        }
        _ => {}
    }
}

// --------------------------------------------------------- prop_testchamber_door

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct ChamberDoor {
    pub open: bool,
    pub locked: bool,
}

fn spawn_chamber_door(world: &mut World, e: Entity) {
    attach_entity_model(world, e, true);
    if let Some(mut anim) = world.get_mut::<StudioAnimator>(e) {
        anim.set_sequence("idleclose");
    }
    world.entity_mut(e).insert(ChamberDoor { open: false, locked: false });
}

fn chamber_door_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let Some(door) = world.get::<ChamberDoor>(e) else { return false };
    let (open, locked) = (door.open, door.locked);
    match call.input.as_str() {
        "open" if !open && !locked => {
            set_door(world, e, true);
            fire_output(world, e, "OnOpen", call.activator, None);
        }
        "close" if open && !locked => {
            set_door(world, e, false);
            fire_output(world, e, "OnClose", call.activator, None);
        }
        "open" | "close" => {}
        "lock" | "unlock" => {
            if let Some(mut d) = world.get_mut::<ChamberDoor>(e) {
                d.locked = call.input == "lock";
            }
        }
        _ => return false,
    }
    true
}

fn set_door(world: &mut World, e: Entity, open: bool) {
    if let Some(mut d) = world.get_mut::<ChamberDoor>(e) {
        d.open = open;
    }
    if let Some(mut anim) = world.get_mut::<StudioAnimator>(e) {
        anim.set_sequence(if open { "open" } else { "close" });
        anim.set_default_sequence(if open { "idleopen" } else { "idleclose" });
    }
    // Fully open/closed is reported when the sequence ends.
    world.entity_mut(e).insert(PendingDoorReport { open });
}

/// Door waiting for its open/close animation to finish.
#[derive(Component, serde::Serialize, serde::Deserialize)]
struct PendingDoorReport {
    open: bool,
}

// ------------------------------------------------------------ pedestal buttons

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct PedestalButton {
    pub pressed_until: Option<f64>,
    pub delay: f32,
    pub locked: bool,
}

fn spawn_pedestal_button(world: &mut World, e: Entity) {
    attach_entity_model(world, e, true);
    let delay = world.get::<MapEntity>(e).and_then(|m| m.get_f32("delay")).unwrap_or(1.0);
    world.entity_mut(e).insert(PedestalButton { pressed_until: None, delay, locked: false });
}

fn pedestal_button_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let now = world.resource::<GameClock>().curtime;
    let Some(b) = world.get::<PedestalButton>(e) else { return false };
    let (busy, locked, delay) = (b.pressed_until.is_some(), b.locked, b.delay);
    let input = if call.input == "use" { "press" } else { call.input.as_str() };
    match input {
        "press" if !busy && !locked => {
            if let Some(mut b) = world.get_mut::<PedestalButton>(e) {
                b.pressed_until = Some(now + delay as f64);
            }
            if let Some(mut anim) = world.get_mut::<StudioAnimator>(e) {
                anim.set_sequence("down");
            }
            crate::audio::emit(world, "Portal.button_down", crate::audio::Emit { entity: Some(e), ..default() });
            fire_output(world, e, "OnPressed", call.activator, None);
        }
        "press" if locked && !busy => {
            crate::audio::emit(world, "Portal.button_locked", crate::audio::Emit { entity: Some(e), ..default() });
        }
        "press" => {}
        "lock" | "unlock" => {
            if let Some(mut b) = world.get_mut::<PedestalButton>(e) {
                b.locked = call.input == "lock";
            }
        }
        "cancelpress" => {
            if let Some(mut b) = world.get_mut::<PedestalButton>(e) {
                b.pressed_until = Some(now);
            }
        }
        _ => return false,
    }
    true
}

fn button_timers(
    clock: Res<GameClock>,
    mut buttons: Query<(Entity, &mut PedestalButton, Option<&mut StudioAnimator>)>,
    mut doors: Query<(Entity, &PendingDoorReport, &StudioAnimator), Without<PedestalButton>>,
    mut commands: Commands,
    mut out: MessageWriter<FireOutput>,
    mut sounds: MessageWriter<crate::audio::SoundRequest>,
) {
    for (e, mut b, anim) in &mut buttons {
        if b.pressed_until.is_some_and(|t| clock.due(t)) {
            b.pressed_until = None;
            if let Some(mut anim) = anim {
                anim.set_sequence("up");
            }
            sounds.write(crate::audio::SoundRequest::on("Portal.button_up", e));
            out.write(FireOutput::new(e, "OnButtonReset"));
        }
    }
    for (e, pending, anim) in &mut doors {
        let name = anim.sequence_name().to_ascii_lowercase();
        let done = anim.finished || name.starts_with("idle");
        if done {
            out.write(FireOutput::new(e, if pending.open { "OnFullyOpen" } else { "OnFullyClosed" }));
            commands.entity(e).remove::<PendingDoorReport>();
        }
    }
}

// --------------------------------------------------------------- floor buttons

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct FloorButton {
    pub pressed: bool,
    /// Last time something rested on the button (seconds).
    pub last_contact: f64,
}

fn spawn_floor_button(world: &mut World, e: Entity) {
    attach_entity_model(world, e, true);
    world.entity_mut(e).insert(FloorButton { pressed: false, last_contact: f64::MIN });
}

/// Floor buttons are pressed by whatever rests on them; until collision exists
/// they react only to these inputs.
fn floor_button_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let press = match call.input.as_str() {
        "pressin" => true,
        "pressout" => false,
        _ => return false,
    };
    set_floor_button(world, e, press, call.activator);
    true
}

pub fn set_floor_button(world: &mut World, e: Entity, pressed: bool, activator: Option<Entity>) {
    let Some(mut b) = world.get_mut::<FloorButton>(e) else { return };
    if b.pressed == pressed {
        return;
    }
    b.pressed = pressed;
    if let Some(mut anim) = world.get_mut::<StudioAnimator>(e) {
        anim.set_sequence(if pressed { "down" } else { "up" });
    }
    crate::audio::emit(
        world,
        if pressed { "Portal.button_down" } else { "Portal.button_up" },
        crate::audio::Emit { entity: Some(e), ..default() },
    );
    fire_output(world, e, if pressed { "OnPressed" } else { "OnUnPressed" }, activator, None);
}

// ------------------------------------------------------------ physics props

/// Weighted cubes and physics props: drawn now, simulated once physics lands.
#[derive(Component)]
pub struct PhysicsProp;

pub(crate) fn spawn_physics_prop(world: &mut World, e: Entity) {
    attach_entity_model(world, e, false);
    world.entity_mut(e).insert(PhysicsProp);
}

// ------------------------------------------------------------- static props

type StaticPropParams<'w, 's> = (
    Commands<'w, 's>,
    Res<'w, GameFs>,
    Res<'w, CurrentMap>,
    ResMut<'w, ModelCache>,
    ResMut<'w, MaterialCache>,
    ResMut<'w, TextureCache>,
    ResMut<'w, Assets<Image>>,
    ResMut<'w, Assets<Mesh>>,
    ResMut<'w, Assets<ModelMaterial>>,
    ResMut<'w, Assets<RefractMaterial>>,
    ResMut<'w, Assets<SkinnedMeshInverseBindposes>>,
);

/// Spawns the map's static props with their baked per-vertex lighting.
pub fn spawn_static_props(world: &mut World) {
    if world.resource::<portal2_core::args::Args>().no_static_props {
        return;
    }
    let mut state: SystemState<StaticPropParams> = SystemState::new(world);
    let (mut commands, fs, map, mut cache, mut mats, mut textures, mut images, mut meshes, mut model_mats, mut refract_mats, mut bindposes) =
        state.get_mut(world).expect("render resources available");
    let props = &map.bsp.static_props;
    let mut missing_lighting = 0;
    for (i, prop) in props.props.iter().enumerate() {
        let Some(path) = props.models.get(prop.model as usize) else { continue };
        let Some(model) = cache.get(path, &fs, &mut meshes, &mut bindposes) else { continue };
        let vhv = fs.read(&format!("sp_hdr_{i}.vhv")).or_else(|| fs.read(&format!("sp_{i}.vhv"))).and_then(|b| Vhv::parse(&b).ok());
        let light_origin = static_prop_lighting_origin(prop, &model.studio);
        let lighting = if vhv.is_none() {
            missing_lighting += 1;
            Some(lighting::compute(&map.bsp, light_origin))
        } else {
            None
        };
        let envmap = lighting::nearest_cubemap(&map.bsp, &map.name, light_origin, &fs, &mut textures, &mut images);
        let transform = Transform::from_translation(prop.origin)
            .with_rotation(angles_to_quat(prop.angles))
            .with_scale(Vec3::splat(prop.uniform_scale.max(0.001)));
        let root = commands.spawn((Name::new(format!("static prop {i}")), MapObject, transform, Visibility::Visible)).id();
        let mut ctx = MaterialContext { fs: &fs, textures: &mut textures, images: &mut images };
        spawn_model_parts(
            &mut commands,
            root,
            SpawnModel { model: &model, skin: prop.skin.max(0) as usize, body: 0, vhv: vhv.as_ref(), animated: false, lighting, envmap },
            &fs,
            &mut ctx,
            &mut mats,
            &mut meshes,
            &mut model_mats,
            &mut refract_mats,
        );
    }
    info!("static props: {} ({} without baked lighting)", props.props.len(), missing_lighting);
    state.apply(world);
}

/// Static prop lump flag: light the prop from its stored lighting origin.
const STATIC_PROP_USE_LIGHTING_ORIGIN: u8 = 2;

/// Where the engine lights a static prop and picks its cube map: the lump's
/// lighting origin when the prop is flagged to use it, otherwise the model's
/// illumination point placed with the prop (its scale plays no part).
fn static_prop_lighting_origin(prop: &StaticProp, studio: &StudioModel) -> Vec3 {
    if prop.flags & STATIC_PROP_USE_LIGHTING_ORIGIN != 0 {
        prop.lighting_origin
    } else {
        prop.origin + angles_to_quat(prop.angles) * studio.mdl.illum_position
    }
}

impl portal2_core::save::Restore for ChamberDoor {}

impl portal2_core::save::Restore for FloorButton {}

impl portal2_core::save::Restore for PedestalButton {}

impl portal2_core::save::Restore for PendingDoorReport {}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<ChamberDoor>("chamber_door");
    r.component::<FloorButton>("floor_button");
    r.component::<PedestalButton>("pedestal_button");
    r.component::<PendingDoorReport>("pending_door_report");
}
