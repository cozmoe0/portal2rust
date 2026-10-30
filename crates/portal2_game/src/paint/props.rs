//! Paint on entities. Blobs that hit a physics prop paint it: it takes the
//! paint's colour, bounce gel makes it spring off whatever it hits (the
//! player's bounce push, plus a random sideways nudge on floors so it does
//! not bounce in place), speed gel takes its friction away, erase cleans
//! it. Other entities hit by paint just take on the paint's colour, as the
//! game sets their render colour. Rules in docs/re/paint.md.

use avian3d::prelude::{Friction, LinearVelocity};
use bevy::prelude::*;

use super::power::GelSettings;
use crate::entities::props::PhysicsProp;
use portal2_core::console::Console;
use portal2_core::sets::ThinkSet;
use portal2_render::source_material::ModelMaterial;
use portal2_world::collision::{Collider, Physics, mask};
use portal2_world::paint_map::PaintType;

const TICK: f32 = 1.0 / 60.0;
/// A painted prop bounces at most this often.
const BOUNCE_COOLDOWN: f32 = 0.1;
/// Coming into a surface slower than this does not bounce.
const MIN_INTO_SPEED: f32 = 20.0;
/// Random sideways push of a floor bounce, either way on each axis.
const FLOOR_JITTER: f32 = 80.0;

pub struct PaintedPropsPlugin;

impl Plugin for PaintedPropsPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<PaintEntity>()
            .add_systems(FixedUpdate, (clean_props, apply_paint, bounce_props).chain().in_set(ThinkSet))
            .add_systems(PostUpdate, tint_painted);
    }
}

/// Paint landing on an entity.
#[derive(Message, Clone, Copy, Debug)]
pub struct PaintEntity {
    pub entity: Entity,
    pub paint: PaintType,
}

/// A prop's gel.
#[derive(Component, Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct PaintedProp {
    pub paint: PaintType,
    #[serde(default)]
    next_bounce: f32,
}

/// The colour an entity is drawn with (paint landed on it); `None` restores it.
#[derive(Component, Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PaintTint(pub Option<[u8; 3]>);

impl portal2_core::save::Restore for PaintedProp {}
impl portal2_core::save::Restore for PaintTint {}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<PaintedProp>("painted_prop");
    r.component::<PaintTint>("paint_tint");
}

fn apply_paint(
    mut commands: Commands,
    mut events: MessageReader<PaintEntity>,
    console: Res<Console>,
    props: Query<(Has<PhysicsProp>, Option<&PaintedProp>)>,
) {
    for ev in events.read() {
        let Ok((is_prop, painted)) = props.get(ev.entity) else { continue };
        let mut e = commands.entity(ev.entity);
        if ev.paint == PaintType::Erase {
            e.try_remove::<PaintedProp>();
            e.try_insert(PaintTint(None));
            e.try_insert(Friction::new(0.8));
            continue;
        }
        let c = ev.paint.color(&console);
        e.try_insert(PaintTint(Some([c[0], c[1], c[2]])));
        if !is_prop || painted.is_some_and(|p| p.paint == ev.paint) {
            continue;
        }
        debug!("prop {} painted {:?}", ev.entity, ev.paint);
        e.try_insert(PaintedProp { paint: ev.paint, next_bounce: 0.0 });
        e.try_insert(Friction::new(if ev.paint == PaintType::Speed { 0.0 } else { 0.8 }));
    }
}

/// Painted props inside an enabled `trigger_paint_cleanser` are washed.
fn clean_props(
    physics: Physics,
    triggers: Query<(Entity, &crate::entities::triggers::Trigger)>,
    props: Query<(Entity, &GlobalTransform), Or<(With<PaintedProp>, With<PaintTint>)>>,
    tints: Query<&PaintTint>,
    mut paint: MessageWriter<PaintEntity>,
) {
    let cleansers: Vec<Entity> = triggers
        .iter()
        .filter(|(_, t)| t.enabled && matches!(t.kind, crate::entities::triggers::TriggerKind::PaintCleanser))
        .map(|(e, _)| e)
        .collect();
    if cleansers.is_empty() {
        return;
    }
    let r = Vec3::splat(16.0);
    for (e, gt) in &props {
        if tints.get(e).is_ok_and(|t| t.0.is_none()) {
            continue;
        }
        if cleansers.iter().any(|&c| physics.touches(c, gt.translation(), -r, r)) {
            paint.write(PaintEntity { entity: e, paint: PaintType::Erase });
        }
    }
}

/// A bounce-painted prop's velocity after hitting a surface with `normal`:
/// the player's bounce push off it, with a random sideways nudge on floors.
fn prop_bounce(v: Vec3, normal: Vec3, g: &GelSettings, jitter: Vec2) -> Vec3 {
    let tangent = v - normal * v.dot(normal);
    let flat = tangent - Vec3::Z * tangent.z;
    let d = normal.z;
    let mut push = if d <= -0.1 {
        let side = (normal - Vec3::Z * d).normalize_or_zero();
        let k = normal.dot(side);
        side * g.wall_up_speed * k + normal * g.min_speed * ((1.0 - k) * (1.0 - g.wall_trade) + g.wall_trade)
    } else {
        let k = 1.0 - d;
        Vec3::Z * g.wall_up_speed * k + normal * g.min_speed * ((1.0 - k) * (1.0 - g.wall_trade) + g.wall_trade)
    };
    if push.x == 0.0 && push.y == 0.0 {
        push.x += jitter.x;
        push.y += jitter.y;
    }
    let along = flat.normalize_or_zero() * flat.dot(push.normalize_or_zero());
    flat - along + push
}

fn bounce_props(
    time: Res<Time<Fixed>>,
    console: Res<Console>,
    physics: Physics,
    mut props: Query<(Entity, &mut PaintedProp, &GlobalTransform, &mut LinearVelocity, Option<&Collider>)>,
    mut rng: Local<u32>,
) {
    let now = time.elapsed_secs();
    let g = GelSettings::from_console(&console);
    for (e, mut painted, gt, mut v, collider) in &mut props {
        if painted.paint != PaintType::Bounce || now < painted.next_bounce {
            continue;
        }
        let speed = v.0.length();
        if speed < MIN_INTO_SPEED {
            continue;
        }
        let reach = collider.map(|c| (c.shape.maxs - c.shape.mins).max_element() * 0.5).unwrap_or(16.0);
        let center = gt.translation();
        let tr = physics.trace(center, center + v.0 / speed * (speed * TICK * 1.5 + reach), Vec3::ZERO, Vec3::ZERO, mask::SOLID_ALL, &[e]);
        if !tr.hit() || tr.start_solid || v.0.dot(tr.normal) > -MIN_INTO_SPEED {
            continue;
        }
        let mut next = || {
            *rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (*rng >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        };
        let jitter = Vec2::new(next(), next()) * FLOOR_JITTER;
        v.0 = prop_bounce(v.0, tr.normal, &g, jitter);
        debug!("painted prop {e} bounced off {} -> {}", tr.normal, v.0);
        painted.next_bounce = now + BOUNCE_COOLDOWN;
    }
}

/// Gives the meshes under a tinted entity their own materials in its colour.
fn tint_painted(
    tinted: Query<(Entity, &PaintTint), Changed<PaintTint>>,
    children: Query<&Children>,
    mut meshes: Query<&mut MeshMaterial3d<ModelMaterial>>,
    mut materials: ResMut<Assets<ModelMaterial>>,
    mut originals: Local<std::collections::HashMap<Entity, Handle<ModelMaterial>>>,
) {
    for (root, tint) in &tinted {
        for e in std::iter::once(root).chain(children.iter_descendants(root)) {
            let Ok(mut handle) = meshes.get_mut(e) else { continue };
            let original = originals.entry(e).or_insert_with(|| handle.0.clone()).clone();
            match tint.0 {
                None => handle.0 = original,
                Some(rgb) => {
                    let Some(mut m) = materials.get(&original).cloned() else { continue };
                    let c = Color::srgb_u8(rgb[0], rgb[1], rgb[2]).to_linear();
                    m.params.color = Vec4::new(c.red, c.green, c.blue, m.params.color.w);
                    handle.0 = materials.add(m);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn painted_cube_bounces_off_the_floor_with_a_nudge() {
        let g = GelSettings::default();
        let v = prop_bounce(Vec3::new(0.0, 0.0, -300.0), Vec3::Z, &g, Vec2::new(40.0, -20.0));
        assert!((v.z - 500.0).abs() < 1e-3);
        assert!((v.x - 40.0).abs() < 1e-3 && (v.y + 20.0).abs() < 1e-3);
        // Off a wall: out and up.
        let v = prop_bounce(Vec3::new(-200.0, 0.0, 0.0), Vec3::X, &g, Vec2::ZERO);
        assert!((v.x - 365.0).abs() < 1e-2 && (v.z - 275.0).abs() < 1e-2);
    }
}
