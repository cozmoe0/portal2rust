//! Entity handles (`CBaseEntity` and subclasses) and the `Entities`
//! iterator.

use bevy::prelude::*;
use squirrel::value::NativeFn;
use squirrel::{Ctx, Error, ObjRef, Result, Value, Vm};

use super::api::{arg, arg_f32, arg_string, arg_vec, make_vec, parts};
use super::{ScriptClasses, ScriptScope};
use crate::coords::{angles_forward, angles_to_quat, quat_to_angles};
use crate::entities::io::{Connection, Outputs};
use crate::entities::{EntityRegistry, MapEntity, world_transform};

pub const ENTITY_TAG: u32 = 0x454e_5449;

/// The entity a handle refers to, if it still exists.
pub fn entity_of(vm: &Vm, world: &World, v: &Value) -> Option<Entity> {
    let Value::Instance(i) = v else { return None };
    let inst = vm.heap.instance(*i);
    if vm.heap.class(inst.class).type_tag != ENTITY_TAG {
        return None;
    }
    let e = Entity::try_from_bits(inst.user)?;
    world.get_entity(e).is_ok().then_some(e)
}

fn this_entity(vm: &Vm, world: &World, args: &[Value]) -> Result<Entity> {
    entity_of(vm, world, &arg(args, 0)).ok_or_else(|| Error::runtime("Accessed null instance"))
}

/// Script class for an entity.
pub fn class_for(world: &World, e: Entity, c: &ScriptClasses) -> ObjRef {
    if world.resource::<EntityRegistry>().player == Some(e) {
        return c.portal_player;
    }
    let class = world.get::<MapEntity>(e).map(|m| m.classname.as_str()).unwrap_or("");
    match class {
        "logic_choreographed_scene" => c.scene,
        "point_viewcontrol" => c.camera,
        "linked_portal_door" => c.linked_door,
        _ if class.starts_with("prop_") || class.starts_with("npc_") || class == "generic_actor" => c.flex,
        _ => c.entity,
    }
}

fn new_class(vm: &mut Vm, name: &str, base: Option<ObjRef>, methods: &[(&str, NativeFn, i32)]) -> ObjRef {
    let Value::Class(class) = vm.new_class(base, ENTITY_TAG) else { unreachable!() };
    for (n, f, p) in methods {
        let native = vm.new_native(n, *f, *p, Value::Null);
        let _ = vm.class_new_slot(class, Value::str(n), native, false, Value::Null);
    }
    let root = vm.root_value();
    vm.raw_set(&root, Value::str(name), Value::Class(class));
    class
}

pub fn register(vm: &mut Vm, vector: ObjRef) -> ScriptClasses {
    let entity = new_class(
        vm,
        "CBaseEntity",
        None,
        &[
            ("GetName", get_name, 1),
            ("GetClassname", get_classname, 1),
            ("GetPreTemplateName", get_name, 1),
            ("GetScriptId", get_script_id, 1),
            ("GetScriptScope", get_script_scope, 1),
            ("ValidateScriptScope", validate_script_scope, 1),
            ("GetOrigin", get_origin, 1),
            ("SetOrigin", set_origin, 2),
            ("SetAbsOrigin", set_origin, 2),
            ("GetAngles", get_angles, 1),
            ("SetAngles", set_angles, 4),
            ("GetForwardVector", get_forward, 1),
            ("SetForwardVector", set_forward, 2),
            ("GetLeftVector", get_left, 1),
            ("GetUpVector", get_up, 1),
            ("GetCenter", get_origin, 1),
            ("EyePosition", eye_position, 1),
            ("GetVelocity", get_velocity, 1),
            ("SetVelocity", set_velocity, 2),
            ("GetAngularVelocity", get_angular_velocity, 1),
            ("SetAngularVelocity", ignore, 4),
            ("GetModelKeyValues", null_fn, 1),
            ("GetBoundingMins", get_mins, 1),
            ("GetBoundingMaxs", get_maxs, 1),
            ("SetSize", ignore, 3),
            ("GetHealth", get_health, 1),
            ("SetHealth", set_health, 2),
            ("GetMaxHealth", get_max_health, 1),
            ("SetMaxHealth", ignore, 2),
            ("GetMoveParent", get_move_parent, 1),
            ("GetRootMoveParent", get_root_move_parent, 1),
            ("FirstMoveChild", first_move_child, 1),
            ("NextMovePeer", next_move_peer, 1),
            ("GetOwner", null_fn, 1),
            ("SetOwner", ignore, 2),
            ("GetTeam", get_team, 1),
            ("SetTeam", set_team, 2),
            ("GetModelName", get_model_name, 1),
            ("SetModel", set_model, 2),
            ("EmitSound", emit_sound, 2),
            ("StopSound", stop_sound, 2),
            ("PrecacheSoundScript", ignore, 2),
            ("GetSoundDuration", sound_duration, 3),
            ("ConnectOutput", connect_output, 3),
            ("DisconnectOutput", disconnect_output, 3),
            ("Destroy", destroy, 1),
            ("IsValid", is_valid, 1),
            ("entindex", entindex, 1),
            ("__KeyValueFromString", key_value, 3),
            ("__KeyValueFromInt", key_value, 3),
            ("__KeyValueFromFloat", key_value, 3),
            ("__KeyValueFromVector", key_value_vector, 3),
            ("_tostring", to_string, 1),
        ],
    );
    let animating = new_class(
        vm,
        "CBaseAnimating",
        Some(entity),
        &[
            ("GetAttachmentOrigin", get_attachment_origin, 2),
            ("GetAttachmentAngles", get_attachment_angles, 2),
            ("LookupAttachment", lookup_attachment, 2),
            ("GetObjectScaleLevel", zero_int, 1),
            ("SetBodygroup", set_bodygroup, 3),
            ("IsSequenceFinished", true_fn, 1),
        ],
    );
    let flex = new_class(vm, "CBaseFlex", Some(animating), &[("GetCurrentScene", get_current_scene, 1), ("GetSceneByIndex", get_scene_by_index, 2)]);
    let player = new_class(vm, "CBasePlayer", Some(animating), &[("IsNoclipping", is_noclipping, 1)]);
    let portal_player = new_class(
        vm,
        "CPortal_Player",
        Some(player),
        &[
            ("GetWheatleyMonitorDestructionCount", zero_int, 1),
            ("IncWheatleyMonitorDestructionCount", ignore, 1),
            ("TurnOffPotatos", ignore, 1),
            ("TurnOnPotatos", ignore, 1),
        ],
    );
    let scene = new_class(
        vm,
        "CSceneEntity",
        Some(entity),
        &[
            ("EstimateLength", estimate_length, 1),
            ("IsPlayingBack", is_playing_back, 1),
            ("IsPaused", is_paused, 1),
            ("AddBroadcastTeamTarget", ignore, 2),
            ("RemoveBroadcastTeamTarget", ignore, 2),
            ("FindNamedEntity", find_named_entity, 2),
            ("LoadSceneFromString", false_fn, 3),
        ],
    );
    let camera = new_class(vm, "CTriggerCamera", Some(entity), &[("GetFov", get_fov, 1), ("SetFov", ignore, 3)]);
    let linked_door = new_class(
        vm,
        "CPropLinkedPortalDoor",
        Some(animating),
        &[("GetPartnerInstance", get_partner_instance, 1), ("GetPartnername", get_partnername, 1)],
    );
    // `Entities` is an instance of CEntities.
    let entities = new_class(
        vm,
        "CEntities",
        None,
        &[
            ("First", first, 1),
            ("Next", next, 2),
            ("FindByName", find_by_name, 3),
            ("FindByClassname", find_by_classname, 3),
            ("FindByModel", find_by_model, 3),
            ("FindByTarget", find_by_target, 3),
            ("FindInSphere", find_in_sphere, 4),
            ("FindByNameNearest", find_by_name_nearest, 4),
            ("FindByNameWithin", find_by_name_within, 5),
            ("FindByClassnameNearest", find_by_classname_nearest, 4),
            ("FindByClassnameWithin", find_by_classname_within, 5),
            ("CreateByClassname", create_by_classname, 2),
        ],
    );
    vm.heap.class_mut(entities).type_tag = 0;
    let inst = vm.new_instance(entities, 0);
    let root = vm.root_value();
    vm.raw_set(&root, Value::str("Entities"), inst);
    ScriptClasses { vector, entity, animating, flex, player, portal_player, scene, camera, linked_door }
}

fn ignore(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Null)
}

fn null_fn(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Null)
}

fn true_fn(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Bool(true))
}

fn false_fn(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Bool(false))
}

fn zero_int(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Int(0))
}

fn zero_float(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Float(0.0))
}

fn get_name(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let name = world.get::<MapEntity>(e).and_then(|m| m.targetname.clone()).unwrap_or_default();
    Ok(Value::str(&name))
}

fn get_classname(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let player = world.resource::<EntityRegistry>().player == Some(e);
    let class = if player { "player".to_owned() } else { world.get::<MapEntity>(e).map(|m| m.classname.clone()).unwrap_or_default() };
    Ok(Value::str(&class))
}

fn get_script_id(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    super::validate_scope(world, vm, e);
    let id = world.get::<ScriptScope>(e).map(|s| s.id.clone()).unwrap_or_default();
    Ok(Value::str(&id))
}

fn get_script_scope(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    Ok(super::scope_of(world, e).unwrap_or_default())
}

fn validate_script_scope(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    super::validate_scope(world, vm, e);
    Ok(Value::Bool(true))
}

fn player_origin(world: &World, e: Entity) -> Option<Vec3> {
    (world.resource::<EntityRegistry>().player == Some(e)).then(|| world.get::<crate::player::Player>(e).map(|p| p.state.origin)).flatten()
}

fn origin_of(world: &World, e: Entity) -> Vec3 {
    player_origin(world, e).unwrap_or_else(|| world_transform(world, e).translation())
}

fn get_origin(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let o = origin_of(world, e);
    Ok(make_vec(vm, world, o))
}

/// Moves an entity to a world position (players move their feet there).
fn set_world_origin(world: &mut World, e: Entity, pos: Vec3) {
    if world.resource::<EntityRegistry>().player == Some(e) {
        if let Some(mut p) = world.get_mut::<crate::player::Player>(e) {
            p.state.origin = pos;
            p.previous_origin = pos;
        }
        return;
    }
    let parent = world.get::<ChildOf>(e).map(|c| c.parent());
    let local = match parent {
        Some(p) => world_transform(world, p).affine().inverse().transform_point3(pos),
        None => pos,
    };
    if let Some(mut t) = world.get_mut::<Transform>(e) {
        t.translation = local;
    }
}

fn set_origin(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let pos = arg_vec(vm, args, 1)?;
    set_world_origin(world, e, pos);
    Ok(Value::Null)
}

fn angles_of(world: &World, e: Entity) -> Vec3 {
    if world.resource::<EntityRegistry>().player == Some(e) {
        return world.resource::<crate::player::PlayerInput>().input.angles;
    }
    quat_to_angles(world_transform(world, e).compute_transform().rotation)
}

fn get_angles(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let a = angles_of(world, e);
    Ok(make_vec(vm, world, a))
}

fn set_world_rotation(world: &mut World, e: Entity, angles: Vec3) {
    if world.resource::<EntityRegistry>().player == Some(e) {
        world.resource_mut::<crate::player::PlayerInput>().input.angles = angles;
        return;
    }
    let q = angles_to_quat(angles);
    let parent = world.get::<ChildOf>(e).map(|c| c.parent());
    let local = match parent {
        Some(p) => world_transform(world, p).compute_transform().rotation.inverse() * q,
        None => q,
    };
    if let Some(mut t) = world.get_mut::<Transform>(e) {
        t.rotation = local;
    }
}

fn set_angles(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    set_world_rotation(world, e, Vec3::new(arg_f32(args, 1), arg_f32(args, 2), arg_f32(args, 3)));
    Ok(Value::Null)
}

fn get_forward(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let f = angles_forward(angles_of(world, e));
    Ok(make_vec(vm, world, f))
}

fn get_left(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let l = angles_to_quat(angles_of(world, e)) * Vec3::Y;
    Ok(make_vec(vm, world, l))
}

fn get_up(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let u = angles_to_quat(angles_of(world, e)) * Vec3::Z;
    Ok(make_vec(vm, world, u))
}

fn set_forward(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let f = arg_vec(vm, args, 1)?.normalize_or(Vec3::X);
    let yaw = f.y.atan2(f.x).to_degrees();
    let pitch = -f.z.clamp(-1.0, 1.0).asin().to_degrees();
    set_world_rotation(world, e, Vec3::new(pitch, yaw, 0.0));
    Ok(Value::Null)
}

fn eye_position(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let eye = match world.get::<crate::player::Player>(e) {
        Some(p) => p.state.origin + Vec3::Z * p.state.eye_offset(),
        None => origin_of(world, e),
    };
    Ok(make_vec(vm, world, eye))
}

fn get_velocity(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let v = world
        .get::<crate::player::Player>(e)
        .map(|p| p.state.velocity)
        .or_else(|| world.get::<avian3d::prelude::LinearVelocity>(e).map(|v| v.0))
        .unwrap_or(Vec3::ZERO);
    Ok(make_vec(vm, world, v))
}

fn set_velocity(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let v = arg_vec(vm, args, 1)?;
    if let Some(mut p) = world.get_mut::<crate::player::Player>(e) {
        p.state.velocity = v;
    } else if let Some(mut lv) = world.get_mut::<avian3d::prelude::LinearVelocity>(e) {
        lv.0 = v;
    }
    Ok(Value::Null)
}

fn get_angular_velocity(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let w = world.get::<avian3d::prelude::AngularVelocity>(e).map(|w| w.0 * (180.0 / std::f32::consts::PI)).unwrap_or(Vec3::ZERO);
    Ok(make_vec(vm, world, w))
}

fn bounds(world: &World, e: Entity) -> (Vec3, Vec3) {
    if let Some(p) = world.get::<crate::player::Player>(e) {
        return p.state.hull();
    }
    world.get::<crate::collision::Collider>(e).map(|c| (c.shape.mins, c.shape.maxs)).unwrap_or((Vec3::ZERO, Vec3::ZERO))
}

fn get_mins(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let b = bounds(world, e).0;
    Ok(make_vec(vm, world, b))
}

fn get_maxs(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let b = bounds(world, e).1;
    Ok(make_vec(vm, world, b))
}

fn get_health(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let h = match world.get::<crate::player::health::Health>(e) {
        Some(h) => h.current as i32,
        None => world.get::<MapEntity>(e).and_then(|m| m.get_i32("health")).unwrap_or(0),
    };
    Ok(Value::Int(h))
}

fn set_health(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let v = arg_f32(args, 1);
    if let Some(mut h) = world.get_mut::<crate::player::health::Health>(e) {
        h.current = v;
    } else if let Some(mut m) = world.get_mut::<MapEntity>(e) {
        m.set("health", &(v as i32).to_string());
    }
    Ok(Value::Null)
}

fn get_max_health(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let max = if world.get::<crate::player::health::Health>(e).is_some() {
        crate::player::health::MAX_HEALTH as i32
    } else {
        world.get::<MapEntity>(e).and_then(|m| m.get_i32("max_health")).unwrap_or(0)
    };
    Ok(Value::Int(max))
}

fn handle_or_null(vm: &mut Vm, world: &mut World, e: Option<Entity>) -> Value {
    match e {
        Some(e) if world.get::<MapEntity>(e).is_some() || world.resource::<EntityRegistry>().player == Some(e) => super::handle_in(world, vm, e),
        _ => Value::Null,
    }
}

fn get_move_parent(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let p = world.get::<ChildOf>(e).map(|c| c.parent());
    Ok(handle_or_null(vm, world, p))
}

fn get_root_move_parent(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let mut e = this_entity(vm, world, args)?;
    while let Some(p) = world.get::<ChildOf>(e).map(|c| c.parent()) {
        if world.get::<MapEntity>(p).is_none() {
            break;
        }
        e = p;
    }
    Ok(handle_or_null(vm, world, Some(e)))
}

fn map_children(world: &World, e: Entity) -> Vec<Entity> {
    world.get::<Children>(e).map(|c| c.iter().filter(|c| world.get::<MapEntity>(*c).is_some()).collect()).unwrap_or_default()
}

fn first_move_child(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let c = map_children(world, e).first().copied();
    Ok(handle_or_null(vm, world, c))
}

fn next_move_peer(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let Some(parent) = world.get::<ChildOf>(e).map(|c| c.parent()) else { return Ok(Value::Null) };
    let peers = map_children(world, parent);
    let next = peers.iter().position(|p| *p == e).and_then(|i| peers.get(i + 1)).copied();
    Ok(handle_or_null(vm, world, next))
}

fn get_team(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let team = world.get::<MapEntity>(e).and_then(|m| m.get_i32("teamnum")).unwrap_or(0);
    Ok(Value::Int(team))
}

fn set_team(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let team = args.get(1).and_then(Value::as_i32).unwrap_or(0);
    if let Some(mut m) = world.get_mut::<MapEntity>(e) {
        m.set("teamnum", &team.to_string());
    }
    Ok(Value::Null)
}

fn get_model_name(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let model = world.get::<MapEntity>(e).and_then(|m| m.get("model").map(str::to_owned)).unwrap_or_default();
    Ok(Value::str(&model))
}

fn set_model(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let model = arg_string(ctx, args, 1)?;
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    if let Some(mut m) = world.get_mut::<MapEntity>(e) {
        m.set("model", &model);
    }
    Ok(Value::Null)
}

fn emit_sound(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let sound = arg_string(ctx, args, 1)?;
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    crate::audio::emit(world, &sound, crate::audio::Emit { entity: Some(e), ..Default::default() });
    Ok(Value::Null)
}

fn stop_sound(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let sound = arg_string(ctx, args, 1)?;
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    crate::audio::stop(world, Some(e), &sound);
    Ok(Value::Null)
}

/// `GetSoundDuration(sound, actormodel)`: length of the entry's wave.
fn sound_duration(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let sound = arg_string(ctx, args, 1)?;
    let (_, world) = parts(ctx);
    Ok(Value::Float(crate::audio::duration(world, &sound)))
}

/// `ConnectOutput(output, function)`: an output connection that calls the
/// function in this entity's scope.
fn connect_output(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let output = arg_string(ctx, args, 1)?.to_ascii_lowercase();
    let func = arg_string(ctx, args, 2)?;
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    if let Some(mut outs) = world.get_mut::<Outputs>(e) {
        let exists = outs.connections.iter().any(|c| {
            c.output == output && c.target == "!self" && c.input == "callscriptfunction" && c.param == func && c.delay == 0.0 && c.times == -1
        });
        if !exists {
            outs.connections.push(Connection {
                output,
                target: "!self".into(),
                input: "callscriptfunction".into(),
                param: func,
                delay: 0.0,
                times: -1,
            });
        }
    }
    Ok(Value::Null)
}

fn disconnect_output(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let output = arg_string(ctx, args, 1)?.to_ascii_lowercase();
    let func = arg_string(ctx, args, 2)?;
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    if let Some(mut outs) = world.get_mut::<Outputs>(e) {
        outs.connections.retain(|c| !(c.output == output && c.target == "!self" && c.input == "callscriptfunction" && c.param == func));
    }
    Ok(Value::Null)
}

/// The oldest scene this actor has an event under way in.
fn get_current_scene(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let scene = crate::entities::scenes::active_scenes(world, e).first().copied();
    Ok(handle_or_null(vm, world, scene))
}

fn get_scene_by_index(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let index = args.get(1).and_then(Value::as_f32).unwrap_or(-1.0);
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let scene = (index >= 0.0).then(|| crate::entities::scenes::active_scenes(world, e).get(index as usize).copied()).flatten();
    Ok(handle_or_null(vm, world, scene))
}

fn destroy(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    crate::entities::despawn_entity(world, e);
    Ok(Value::Null)
}

fn is_valid(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    Ok(Value::Bool(entity_of(vm, world, &arg(args, 0)).is_some()))
}

fn entindex(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    Ok(Value::Int(e.index_u32() as i32))
}

fn key_value(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let key = arg_string(ctx, args, 1)?;
    let value = arg_string(ctx, args, 2)?;
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    if let Some(mut m) = world.get_mut::<MapEntity>(e) {
        m.set(&key, &value);
    }
    Ok(Value::Bool(true))
}

fn key_value_vector(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let key = arg_string(ctx, args, 1)?;
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let v = arg_vec(vm, args, 2)?;
    if let Some(mut m) = world.get_mut::<MapEntity>(e) {
        m.set(&key, &format!("{} {} {}", v.x, v.y, v.z));
    }
    Ok(Value::Bool(true))
}

fn to_string(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    Ok(Value::str(&match entity_of(vm, world, &arg(args, 0)) {
        Some(e) => {
            let class = world.get::<MapEntity>(e).map(|m| m.classname.clone()).unwrap_or_else(|| "player".into());
            let name = world.get::<MapEntity>(e).and_then(|m| m.targetname.clone()).unwrap_or_default();
            format!("([{}] {}: {})", e.index_u32(), class, name)
        }
        None => "(null entity)".to_owned(),
    }))
}

fn get_attachment_origin(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let o = origin_of(world, e);
    Ok(make_vec(vm, world, o))
}

fn get_attachment_angles(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let a = angles_of(world, e);
    Ok(make_vec(vm, world, a))
}

fn lookup_attachment(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Int(0))
}

fn set_bodygroup(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let value = args.get(2).and_then(Value::as_i32).unwrap_or(0);
    if let Some(mut m) = world.get_mut::<MapEntity>(e) {
        m.set("body", &value.to_string());
    }
    Ok(Value::Null)
}

fn is_noclipping(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    Ok(Value::Bool(world.get::<crate::player::Player>(e).is_some_and(|p| p.noclip)))
}

/// Length of the scene in seconds (its last event's end).
fn estimate_length(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    Ok(Value::Float(crate::entities::scenes::loaded(world, e).map_or(0.0, |s| s.duration)))
}

fn is_playing_back(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    Ok(Value::Bool(world.get::<crate::entities::scenes::ChoreoScene>(e).is_some_and(|s| s.playing)))
}

fn is_paused(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    Ok(Value::Bool(world.get::<crate::entities::scenes::ChoreoScene>(e).is_some_and(|s| s.paused)))
}

fn find_named_entity(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let name = arg_string(ctx, args, 1)?.to_ascii_lowercase();
    let (vm, world) = parts(ctx);
    let found = crate::entities::io::resolve_targets(world, &name, None, None).first().copied();
    Ok(handle_or_null(vm, world, found))
}

fn get_fov(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Float(90.0))
}

fn get_partnername(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let name = world.get::<MapEntity>(e).and_then(|m| m.get("partnername").map(str::to_owned)).unwrap_or_default();
    Ok(Value::str(&name))
}

fn get_partner_instance(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let e = this_entity(vm, world, args)?;
    let name = world.get::<MapEntity>(e).and_then(|m| m.get("partnername").map(str::to_ascii_lowercase)).unwrap_or_default();
    let found = world.resource::<EntityRegistry>().first_named(&name);
    Ok(handle_or_null(vm, world, found))
}

// ---------------------------------------------------------------- Entities

/// Entities in the order the iterator visits them: the player, then map
/// entities in map order, then ones created at runtime.
fn iteration_order(world: &mut World) -> Vec<Entity> {
    let player = world.resource::<EntityRegistry>().player;
    let mut q = world.query::<(Entity, &MapEntity)>();
    let mut list: Vec<(usize, u64, Entity)> = q.iter(world).map(|(e, m)| (m.index, e.to_bits(), e)).collect();
    list.sort();
    player.into_iter().chain(list.into_iter().map(|(_, _, e)| e)).collect()
}

fn after<'a>(order: &'a [Entity], prev: Option<Entity>) -> &'a [Entity] {
    match prev.and_then(|p| order.iter().position(|e| *e == p)) {
        Some(i) => &order[i + 1..],
        None => order,
    }
}

fn name_matches(world: &World, e: Entity, pattern: &str) -> bool {
    let is_player = world.resource::<EntityRegistry>().player == Some(e);
    if pattern == "!player" {
        return is_player;
    }
    let Some(name) = world.get::<MapEntity>(e).and_then(|m| m.targetname.clone()) else { return false };
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == pattern,
    }
}

fn class_matches(world: &World, e: Entity, pattern: &str) -> bool {
    let class = if world.resource::<EntityRegistry>().player == Some(e) {
        "player".to_owned()
    } else {
        world.get::<MapEntity>(e).map(|m| m.classname.clone()).unwrap_or_default()
    };
    match pattern.strip_suffix('*') {
        Some(prefix) => class.starts_with(prefix),
        None => class == pattern,
    }
}

fn find_next(ctx: &mut Ctx, args: &[Value], prev_index: usize, pred: impl Fn(&World, Entity) -> bool) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let prev = entity_of(vm, world, &arg(args, prev_index));
    let order = iteration_order(world);
    let found = after(&order, prev).iter().copied().find(|e| pred(world, *e));
    Ok(handle_or_null(vm, world, found))
}

fn first(ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let order = iteration_order(world);
    Ok(handle_or_null(vm, world, order.first().copied()))
}

fn next(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    find_next(ctx, args, 1, |_, _| true)
}

fn find_by_name(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let name = arg_string(ctx, args, 2)?.to_ascii_lowercase();
    find_next(ctx, args, 1, move |w, e| name_matches(w, e, &name))
}

fn find_by_classname(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let class = arg_string(ctx, args, 2)?.to_ascii_lowercase();
    find_next(ctx, args, 1, move |w, e| class_matches(w, e, &class))
}

fn find_by_model(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let model = arg_string(ctx, args, 2)?.to_ascii_lowercase();
    find_next(ctx, args, 1, move |w, e| w.get::<MapEntity>(e).and_then(|m| m.get("model")).is_some_and(|m| m.eq_ignore_ascii_case(&model)))
}

fn find_by_target(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let target = arg_string(ctx, args, 2)?.to_ascii_lowercase();
    find_next(ctx, args, 1, move |w, e| w.get::<MapEntity>(e).and_then(|m| m.get("target")).is_some_and(|t| t.eq_ignore_ascii_case(&target)))
}

fn find_in_sphere(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let center = arg_vec(ctx.vm, args, 2)?;
    let radius = arg_f32(args, 3);
    find_next(ctx, args, 1, move |w, e| origin_of(w, e).distance(center) <= radius)
}

fn find_within(ctx: &mut Ctx, args: &[Value], by_class: bool) -> Result<Value> {
    let pattern = arg_string(ctx, args, 2)?.to_ascii_lowercase();
    let center = arg_vec(ctx.vm, args, 3)?;
    let radius = arg_f32(args, 4);
    find_next(ctx, args, 1, move |w, e| {
        (if by_class { class_matches(w, e, &pattern) } else { name_matches(w, e, &pattern) }) && origin_of(w, e).distance(center) <= radius
    })
}

fn find_by_name_within(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    find_within(ctx, args, false)
}

fn find_by_classname_within(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    find_within(ctx, args, true)
}

fn find_nearest(ctx: &mut Ctx, args: &[Value], by_class: bool) -> Result<Value> {
    let pattern = arg_string(ctx, args, 1)?.to_ascii_lowercase();
    let (vm, world) = parts(ctx);
    let center = arg_vec(vm, args, 2)?;
    let radius = arg_f32(args, 3);
    let order = iteration_order(world);
    let mut best: Option<(f32, Entity)> = None;
    for e in order {
        let ok = if by_class { class_matches(world, e, &pattern) } else { name_matches(world, e, &pattern) };
        if !ok {
            continue;
        }
        let d = origin_of(world, e).distance(center);
        if (radius <= 0.0 || d <= radius) && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, e));
        }
    }
    Ok(handle_or_null(vm, world, best.map(|(_, e)| e)))
}

fn find_by_name_nearest(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    find_nearest(ctx, args, false)
}

fn find_by_classname_nearest(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    find_nearest(ctx, args, true)
}

fn create_by_classname(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let class = arg_string(ctx, args, 1)?;
    let (vm, world) = parts(ctx);
    let e = crate::entities::spawn_runtime_entity(world, &class, &[]);
    Ok(super::handle_in(world, vm, e))
}
