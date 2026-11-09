//! `scripted_sequence`, as far as the campaign uses it: `BeginSequence`
//! plays `m_iszPlay` on the actor named by `m_iszEntity` and then leaves it
//! idling in `m_iszIdle`, firing `OnBeginSequence` and, once the animation
//! has run, `OnEndSequence`. Actors are not walked into place first.
//! `sp_a2_core`'s turrets pop their guns out this way as GLaDOS's turret
//! scene starts.

use bevy::prelude::*;

use super::io::{EventQueue, EventTarget, InputCall, InputHandlers, fire_output};
use super::{EntityRegistry, MapEntity, SpawnHandlers};
use portal2_render::models::StudioAnimator;

pub fn register(app: &mut App) {
    let world = app.world_mut();
    world.resource_mut::<SpawnHandlers>().register("scripted_sequence", |_, _| {});
    world.resource_mut::<InputHandlers>().register("scripted_sequence", sequence_input);
}

fn sequence_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    match call.input.as_str() {
        "beginsequence" => {
            let Some(me) = world.get::<MapEntity>(e) else { return true };
            let key = |k: &str| me.get(k).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
            let (actor, play, idle) = (key("m_iszentity"), key("m_iszplay"), key("m_iszidle"));
            let actors: Vec<Entity> = actor
                .map(|a| world.resource::<EntityRegistry>().by_name.get(&a.to_ascii_lowercase()).cloned().unwrap_or_default())
                .unwrap_or_default();
            let mut length = 0.0f32;
            for a in actors {
                let Some(mut anim) = world.get_mut::<StudioAnimator>(a) else { continue };
                if let Some(idle) = &idle {
                    anim.set_default_sequence(idle);
                }
                if let Some(play) = &play
                    && anim.set_sequence(play)
                {
                    anim.rate = 1.0;
                    length = length.max(anim.duration());
                }
            }
            fire_output(world, e, "OnBeginSequence", call.activator, None);
            world.resource_mut::<EventQueue>().push(length, EventTarget::Entity(e), "__endsequence", "", call.activator, None);
        }
        "__endsequence" => fire_output(world, e, "OnEndSequence", call.activator, None),
        "cancelsequence" | "movetoposition" => {}
        _ => return false,
    }
    true
}
