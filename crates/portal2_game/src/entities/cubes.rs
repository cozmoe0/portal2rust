//! Weighted cubes (`prop_weighted_cube`). The server picks each cube's model
//! and skin itself (server.dll `CPropWeightedCube`, `docs/re/gameplay.md`,
//! weighted cubes): the model from the cube type, the skin from the type,
//! whether it is rusted, its paint and whether it rests on a floor button.

use bevy::prelude::*;

use super::{MapEntity, SpawnHandlers};

/// Models by cube type: standard and companion share one.
const MODELS: [&str; 5] = [
    "models/props/metal_box.mdl",
    "models/props/metal_box.mdl",
    "models/props/reflection_cube.mdl",
    "models/props_gameplay/mp_ball.mdl",
    "models/props_underground/underground_weighted_cube.mdl",
];

pub const REFLECTIVE: i32 = 2;

pub fn register(app: &mut App) {
    app.add_systems(FixedUpdate, update_skins);
    app.world_mut().resource_mut::<SpawnHandlers>().register("prop_weighted_cube", spawn_cube);
}

#[derive(Component, Clone, Copy, Default, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct WeightedCube {
    /// 0 standard, 1 companion, 2 reflective, 3 sphere, 4 antique.
    pub cube_type: i32,
    pub rusted: bool,
    /// Resting on a floor button that takes it.
    pub activated: bool,
}

impl WeightedCube {
    /// The skin for this state; `paint` is the paint power (0 bounce,
    /// 2 speed, anything else none).
    pub fn skin(&self, paint: i32) -> i32 {
        let (bounce, speed, on) = (paint == 0, paint == 2, self.activated);
        match self.cube_type {
            0 if self.rusted => {
                if on {
                    5
                } else {
                    3
                }
            }
            0 if bounce => {
                if on {
                    10
                } else {
                    6
                }
            }
            0 if speed => {
                if on {
                    11
                } else {
                    7
                }
            }
            0 => {
                if on {
                    2
                } else {
                    0
                }
            }
            1 if bounce => 8,
            1 if speed => 9,
            1 => {
                if on {
                    4
                } else {
                    1
                }
            }
            2 | 3 if bounce => 2,
            2 | 3 if speed => 3,
            2 => i32::from(self.rusted),
            3 => i32::from(on),
            4 if bounce => 1,
            4 if speed => 2,
            _ => 0,
        }
    }
}

/// Settles a cube's type from its keyvalues and points `model` at its
/// model. Without `NewSkins` the old `skin` field gives the type: skins
/// above 1 are one more than the type.
pub fn prepare(me: &mut MapEntity) -> WeightedCube {
    let cube_type = if me.get_bool("newskins") {
        me.get_i32("cubetype").unwrap_or(0)
    } else {
        let skin = me.get_i32("skin").unwrap_or(0);
        if skin > 1 { skin - 1 } else { skin }
    }
    .clamp(0, MODELS.len() as i32 - 1);
    let cube = WeightedCube { cube_type, rusted: me.get_bool("skintype"), activated: false };
    me.set("cubetype", &cube_type.to_string());
    me.set("newskins", "1");
    me.set("model", MODELS[cube_type as usize]);
    me.set("skin", &cube.skin(paint_power(me)).to_string());
    cube
}

fn paint_power(me: &MapEntity) -> i32 {
    me.get_i32("paintpower").unwrap_or(4)
}

fn spawn_cube(world: &mut World, e: Entity) {
    let Some(cube) = world.get_mut::<MapEntity>(e).map(|mut me| prepare(&mut me)) else { return };
    world.entity_mut(e).insert(cube);
    super::props::spawn_physics_prop(world, e);
}

/// Changes a cube's skin when its state calls for another.
fn update_skins(mut commands: Commands, cubes: Query<(Entity, &WeightedCube, &MapEntity), Changed<WeightedCube>>) {
    for (e, cube, me) in &cubes {
        let skin = cube.skin(paint_power(me));
        if me.get_i32("skin") != Some(skin) {
            commands.queue(move |world: &mut World| super::props::set_physics_prop_skin(world, e, skin));
        }
    }
}

impl portal2_core::save::Restore for WeightedCube {}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(cube_type: i32, rusted: bool, activated: bool) -> WeightedCube {
        WeightedCube { cube_type, rusted, activated }
    }

    #[test]
    fn skins_follow_the_server() {
        assert_eq!(cube(0, false, false).skin(4), 0);
        assert_eq!(cube(0, false, true).skin(4), 2);
        assert_eq!(cube(0, true, false).skin(4), 3);
        assert_eq!(cube(0, true, true).skin(4), 5);
        assert_eq!(cube(0, false, true).skin(0), 10);
        assert_eq!(cube(1, false, false).skin(4), 1);
        assert_eq!(cube(1, false, true).skin(4), 4);
        assert_eq!(cube(2, true, false).skin(4), 1);
        assert_eq!(cube(2, false, true).skin(4), 0);
        assert_eq!(cube(2, false, false).skin(2), 3);
        assert_eq!(cube(4, false, false).skin(0), 1);
    }

    #[test]
    fn old_skins_give_the_type() {
        let mut me =
            MapEntity { index: 0, classname: "prop_weighted_cube".into(), targetname: None, keyvalues: Vec::new(), defaults: None };
        me.set("skin", "3");
        let c = prepare(&mut me);
        assert_eq!(c.cube_type, REFLECTIVE);
        assert_eq!(me.studio_model().as_deref(), Some("models/props/reflection_cube.mdl"));
        assert_eq!(me.get_i32("skin"), Some(0));
    }
}
