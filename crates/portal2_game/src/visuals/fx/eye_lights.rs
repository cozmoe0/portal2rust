//! Light from the characters' eyes: Wheatley's (and the other personality
//! cores', in any of the bodies they turn up in) and GLaDOS's.
//!
//! Every animated model is checked once for an eye rig, by bone names, so
//! the light follows the eye wherever the model appears: on the management
//! rail, plugged into a socket, carried in the player's hands, in GLaDOS's
//! body. Each frame the eye becomes a soft, wide spot light just in front of
//! its lens, aimed where the eye looks, in the colour of its lens.
//!
//! A core's brightness follows what its eye is doing:
//! - the gap between the eyelids (`eyelid_upper_main` / `eyelid_lower_main`
//!   turn about the eye; about 35° apart when relaxed, 50° wide open,
//!   nothing when shut), so a blink puts the light out and a wide stare
//!   brightens it;
//! - the iris aperture (`inner01` sits 4.6 units off the eye's axis when
//!   open, 2 when it narrows), the lit share of the lens;
//! - a touch of the voice: the game's `LightedMouth` proxy, which tints the
//!   lens (client.dll `sub_103A0190`), rests at 0.8 and follows the speech
//!   level while the core talks; here that only nudges the light.
//!
//! GLaDOS's eye has no lids. A glow sprite on her model (her eye in
//! `sp_a1_wakeup`, which flickers on as she wakes) switches her light with
//! it.
//!
//! `r_fx_eyelights` scales them all (0 switches them off).

use bevy::prelude::*;

use super::lights::{FxLightData, FxLightShape, FxLights, FxProduceSet, light_flags};
use super::{FxSettings, features};
use crate::devices::laser::srgb_to_linear;
use portal2_core::entity::MapEntity;
use portal2_core::view::MainCamera;
use portal2_render::models::StudioAnimator;

/// The lids' gap when a core's eye is relaxed.
const RELAXED_GAP: f32 = 35.0;
/// How far a core's iris blades sit off the eye's axis when wide open.
const OPEN_IRIS: f32 = 4.58;
/// A core's light, on the lightmaps' scale, with its near-field softening
/// (squared distance) and reach, for the full-size core.
const CORE_INTENSITY: f32 = 320.0;
const CORE_SOFTEN: f32 = 900.0;
const CORE_RANGE: f32 = 320.0;
/// The lens is a wide emitter: light falls off from its axis to nothing at
/// 85°, more steeply as the lids narrow the slit.
const CORE_COS_OUTER: f32 = 0.087;
const CORE_EXPONENT: f32 = 1.0;
/// How far in front of the lens the light sits (full-size core), so the
/// lids and shell around the eye do not shadow it.
const CORE_LIGHT_OFFSET: f32 = 2.5;
/// GLaDOS's eye: a narrower, much stronger beam from her lens, which faces
/// the eye bone's -Y about 8 units in front of it (`mdlmesh`: the self-lit
/// lens sits on `headbits_eye_leftright`, facing -Y).
const GLADOS_LENS: Vec3 = Vec3::new(0.0, -9.0, -1.6);
const GLADOS_INTENSITY: f32 = 6000.0;
const GLADOS_SOFTEN: f32 = 2500.0;
const GLADOS_RANGE: f32 = 1100.0;
const GLADOS_COS_OUTER: f32 = 0.5;
const GLADOS_EXPONENT: f32 = 2.0;
/// Her eye glow sprite's colour (`glados_eyeglow`).
const GLADOS_COLOR: [f32; 3] = [1.0, 190.0 / 255.0, 20.0 / 255.0];

/// The lens colours of the cores' skins (alpha-weighted means of their
/// self-illuminated eye textures, sRGB), by eye material name.
const CORE_COLORS: &[(&str, [f32; 3])] = &[
    ("personality_sphere_lights01", [0.100, 0.598, 0.136]),
    ("personality_sphere_lights02", [0.732, 0.549, 0.057]),
    ("personality_sphere_lights03", [0.656, 0.347, 0.514]),
    ("personality_sphere_light_damaged", [0.264, 0.497, 0.761]),
    ("personality_sphere_lights", [0.174, 0.393, 0.832]),
];
/// Wheatley's, when the skin's eye material is not one of those.
const WHEATLEY_COLOR: [f32; 3] = [0.174, 0.393, 0.832];
/// The cores' lens mesh is skin slot 2.
const CORE_LENS_SLOT: usize = 2;

pub struct EyeLightsPlugin;

impl Plugin for EyeLightsPlugin {
    fn build(&self, app: &mut App) {
        {
            let mut console = app.world_mut().resource_mut::<portal2_core::console::Console>();
            console.register_cvar("r_fx_eyelights", "1", portal2_core::console::cvars::flags::ARCHIVE);
        }
        app.add_systems(PostUpdate, (find_rigs, eye_lights).chain().in_set(FxProduceSet));
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Core { upper: Entity, lower: Entity, iris: Option<Entity>, scale: f32 },
    Glados,
}

/// An eye found on a model. `model` is the loaded model it was found on, so
/// a model swap looks again.
#[derive(Component, Clone, Debug)]
pub struct EyeRig {
    model: usize,
    kind: Kind,
    /// The joint the lens rides and where on it the light sits, aimed along
    /// its `axis`.
    joint: Entity,
    offset: Vec3,
    axis: Vec3,
    /// Linear lens colour of unit luminance, and the skin it was taken for.
    color: Vec3,
    skin: i32,
    /// The `LightedMouth` proxy's memory of recent speech.
    speech: f32,
}

/// A model checked and found without an eye.
#[derive(Component)]
pub struct NoEyeRig(usize);

fn model_id(anim: &StudioAnimator) -> usize {
    std::sync::Arc::as_ptr(&anim.model) as usize
}

fn find_rigs(
    settings: Res<FxSettings>,
    models: Query<(Entity, &StudioAnimator, Option<&MapEntity>, Option<&EyeRig>, Option<&NoEyeRig>)>,
    mut commands: Commands,
) {
    if !settings.has(features::LIGHTS) {
        return;
    }
    for (e, anim, me, rig, none) in &models {
        let id = model_id(anim);
        if rig.is_some_and(|r| r.model == id) || none.is_some_and(|n| n.0 == id) {
            continue;
        }
        let skin = me.and_then(|m| m.get_i32("skin")).unwrap_or(0);
        match rig_for(anim, skin) {
            Some(rig) => {
                commands.entity(e).remove::<NoEyeRig>().insert(rig);
            }
            None => {
                commands.entity(e).remove::<EyeRig>().insert(NoEyeRig(id));
            }
        }
    }
}

fn rig_for(anim: &StudioAnimator, skin: i32) -> Option<EyeRig> {
    let mdl = &anim.model.studio.mdl;
    let bone = |name: &str| mdl.bones.iter().position(|b| b.name.eq_ignore_ascii_case(name));
    let joint = |i: usize| anim.joints.get(i).copied();
    let model = model_id(anim);
    if let (Some(upper), Some(lower), Some(a)) =
        (bone("eyelid_upper_main"), bone("eyelid_lower_main"), mdl.find_attachment("eyes").and_then(|i| mdl.attachments.get(i)))
    {
        // The lens faces along the aperture bone's x, where `eyes` sits.
        let lens = a.local.w_axis.truncate();
        let scale = lens.length() / 6.99;
        let iris = bone("inner01").and_then(joint);
        return Some(EyeRig {
            model,
            kind: Kind::Core { upper: joint(upper)?, lower: joint(lower)?, iris, scale },
            joint: joint(a.bone.max(0) as usize)?,
            offset: lens + Vec3::X * CORE_LIGHT_OFFSET * scale,
            axis: Vec3::X,
            color: core_color(mdl, skin),
            skin,
            speech: 0.0,
        });
    }
    let eye = mdl.bones.iter().position(|b| b.name.to_ascii_lowercase().contains("headbits_eye_leftright"))?;
    Some(EyeRig {
        model,
        kind: Kind::Glados,
        joint: joint(eye)?,
        offset: GLADOS_LENS,
        axis: Vec3::NEG_Y,
        color: unit_luminance(GLADOS_COLOR),
        skin,
        speech: 0.0,
    })
}

/// A core's lens colour for a skin, from the material its lens mesh uses.
fn core_color(mdl: &source_formats::mdl::Mdl, skin: i32) -> Vec3 {
    let family = mdl.skins.get(skin.max(0) as usize).or_else(|| mdl.skins.first());
    let texture = family.and_then(|f| f.get(CORE_LENS_SLOT)).map_or(CORE_LENS_SLOT, |&t| t.max(0) as usize);
    let name = mdl.textures.get(texture).map(|t| t.to_ascii_lowercase().replace('\\', "/")).unwrap_or_default();
    let srgb = CORE_COLORS.iter().find(|(n, _)| name.ends_with(n)).map_or(WHEATLEY_COLOR, |(_, c)| *c);
    unit_luminance(srgb)
}

fn unit_luminance(srgb: [f32; 3]) -> Vec3 {
    let c = Vec3::from_array(srgb.map(srgb_to_linear));
    c / c.dot(Vec3::new(0.2126, 0.7152, 0.0722)).max(1e-4)
}

/// Brightness from the lids' gap in degrees: nothing when shut, in step
/// with the opening, up to half as much again when wide.
fn lid_factor(gap_degrees: f32) -> f32 {
    let shut = ((gap_degrees - 1.0) / 5.0).clamp(0.0, 1.0);
    (gap_degrees / RELAXED_GAP).clamp(0.0, 1.5) * shut * shut * (3.0 - 2.0 * shut)
}

/// Brightness from the iris: the lit share of the lens grows with the
/// aperture's area.
fn iris_factor(radius: f32) -> f32 {
    let a = (radius / OPEN_IRIS).clamp(0.0, 1.25);
    0.4 + 0.6 * a * a
}

/// The `LightedMouth` proxy (client.dll `sub_103A0190`): from the mouth
/// level (0-128), the speech memory jumps to three times the level and
/// decays 2% every 1/30 s; the lens shows the level, or 0.8 of what the
/// memory leaves.
fn lighted_mouth(mouth: f32, memory: &mut f32, dt: f32) -> f32 {
    let raw = mouth / 64.0;
    let level = if raw > 1.0 {
        1.0
    } else if raw < 0.2 {
        0.0
    } else {
        raw
    };
    let target = (level * 3.0).min(1.0);
    if target > *memory {
        *memory = target;
    } else {
        *memory *= 0.98f32.powf(30.3 * dt);
    }
    level.max((1.0 - *memory) * 0.8)
}

/// The lids' gap: how far the upper lid's edge has turned from the lower
/// one's, about the upper lid's hinge.
fn lid_gap(upper: &GlobalTransform, lower: &GlobalTransform) -> f32 {
    let (u, l) = (upper.affine().matrix3.x_axis, lower.affine().matrix3.x_axis);
    let hinge = upper.affine().matrix3.z_axis;
    let (u, l, hinge) = (Vec3::from(u), Vec3::from(l), Vec3::from(hinge));
    l.cross(u).dot(hinge).atan2(l.dot(u)).to_degrees()
}

#[allow(clippy::too_many_arguments)]
fn eye_lights(
    settings: Res<FxSettings>,
    console: Res<portal2_core::console::Console>,
    time: Res<Time>,
    sounds: Option<Res<portal2_audio::Sounds>>,
    holding: Res<crate::physics::carry::Holding>,
    physics: portal2_world::collision::Physics,
    camera: Query<&GlobalTransform, With<MainCamera>>,
    mut rigs: Query<(Entity, &mut EyeRig, &StudioAnimator, Option<&MapEntity>, Option<&InheritedVisibility>)>,
    joints: Query<(&GlobalTransform, &Transform)>,
    sprites: Query<(Entity, &crate::visuals::sprites::SourceSprite)>,
    parents: Query<&ChildOf>,
    mut lights: ResMut<FxLights>,
) {
    let scale = console.f32("r_fx_eyelights");
    if !settings.has(features::LIGHTS) || scale <= 0.0 {
        return;
    }
    let dt = time.delta_secs();
    // Glow sprites riding a rig's model switch its eye.
    let mut glows: Vec<(Entity, bool)> = Vec::new();
    for (s, sprite) in &sprites {
        let mut cur = s;
        for _ in 0..8 {
            let Ok(p) = parents.get(cur) else { break };
            cur = p.parent();
            if rigs.contains(cur) {
                let on = sprite.on;
                match glows.iter_mut().find(|(e, _)| *e == cur) {
                    Some(g) => g.1 |= on,
                    None => glows.push((cur, on)),
                }
                break;
            }
        }
    }
    let eye = camera.single().ok().map(|c| c.translation());
    for (e, mut rig, anim, me, shown) in &mut rigs {
        if rig.model != model_id(anim) || shown.is_some_and(|v| !v.get()) {
            continue;
        }
        let Ok((joint, _)) = joints.get(rig.joint) else { continue };
        let skin = me.and_then(|m| m.get_i32("skin")).unwrap_or(0);
        if skin != rig.skin && matches!(rig.kind, Kind::Core { .. }) {
            rig.color = core_color(&anim.model.studio.mdl, skin);
            rig.skin = skin;
        }
        let mut position = joint.transform_point(rig.offset);
        let axis = joint.affine().transform_vector3(rig.axis).normalize_or_zero();
        let mouth = sounds.as_ref().map_or(0.0, |s| s.mouth_open(e));
        let (energy, soften, range, cos_outer, exponent, radius) = match rig.kind {
            Kind::Core { upper, lower, iris, scale: size } => {
                let (Ok((u, _)), Ok((l, _))) = (joints.get(upper), joints.get(lower)) else { continue };
                let lids = lid_factor(lid_gap(u, l));
                let iris = iris.and_then(|j| joints.get(j).ok()).map_or(1.0, |(_, t)| iris_factor(t.translation.length() / size));
                let voice = lighted_mouth(mouth, &mut rig.speech, dt);
                // Resting at 0.8, the voice moves the light by a tenth.
                let voice = 0.92 + 0.1 * voice;
                let narrow = 1.0 - lids.min(1.0);
                (
                    CORE_INTENSITY * size * size * lids * iris * voice,
                    CORE_SOFTEN * size * size,
                    CORE_RANGE * size.max(0.5),
                    CORE_COS_OUTER,
                    CORE_EXPONENT + 2.0 * narrow,
                    4.0 * size,
                )
            }
            Kind::Glados => {
                if glows.iter().find(|(g, _)| *g == e).is_some_and(|(_, on)| !on) {
                    continue;
                }
                (GLADOS_INTENSITY, GLADOS_SOFTEN, GLADOS_RANGE, GLADOS_COS_OUTER, GLADOS_EXPONENT, 12.0)
            }
        };
        if energy <= 0.0 || axis == Vec3::ZERO {
            continue;
        }
        // A core held as a view model is drawn over the world, but its
        // light must not shine from inside the wall in front of the
        // player: it comes from where the view reaches.
        let held = holding.entity == Some(e) && holding.view_model;
        if held && !console.bool("r_drawviewmodel") {
            continue;
        }
        if held && let Some(eye) = eye {
            let t = physics.trace(eye, position, Vec3::ZERO, Vec3::ZERO, portal2_world::collision::mask::SOLID_BRUSH_ONLY, &[e]);
            if t.fraction < 1.0 {
                position = eye + (t.end - eye) * 0.9;
            }
        }
        lights.frame.push(FxLightData {
            position,
            shape: FxLightShape::Spot { axis, cos_inner: 1.0, cos_outer, exponent, attenuation: Vec3::new(soften, 0.0, 1.0) },
            color: rig.color * energy * scale,
            range,
            flags: light_flags::DIFFUSE | light_flags::SPECULAR,
            radius,
            priority: 3.0,
            // A moving light's map is redrawn every frame: only worth it
            // while the view is near enough to see what it lights.
            shadow: eye.is_some_and(|v| v.distance(position) < range * 2.0),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lids_put_the_light_out_and_a_stare_brightens_it() {
        assert_eq!(lid_factor(0.0), 0.0);
        assert_eq!(lid_factor(-4.0), 0.0);
        assert!((lid_factor(RELAXED_GAP) - 1.0).abs() < 1e-5);
        assert!(lid_factor(51.0) > 1.4);
        assert!(lid_factor(24.0) < lid_factor(RELAXED_GAP));
        assert!(iris_factor(OPEN_IRIS) > iris_factor(2.2));
        assert!((iris_factor(OPEN_IRIS) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn lighted_mouth_rests_at_eight_tenths_and_follows_speech() {
        let mut memory = 0.0;
        assert!((lighted_mouth(0.0, &mut memory, 0.016) - 0.8).abs() < 1e-5);
        // Speaking: the lens shows the level, dark between syllables.
        assert!((lighted_mouth(48.0, &mut memory, 0.016) - 0.75).abs() < 1e-5);
        assert_eq!(memory, 1.0);
        assert!(lighted_mouth(4.0, &mut memory, 0.016) < 0.01);
        // Silence brings it back up over a few seconds.
        for _ in 0..600 {
            lighted_mouth(0.0, &mut memory, 1.0 / 60.0);
        }
        assert!(lighted_mouth(0.0, &mut memory, 0.016) > 0.75);
    }

    #[test]
    fn lid_gap_reads_the_angle_between_the_lids() {
        // Lid edges along x, tilted 17.3° up and down about the hinge (z).
        let up = GlobalTransform::from(Transform::from_rotation(Quat::from_rotation_z(17.3f32.to_radians())));
        let down = GlobalTransform::from(Transform::from_rotation(Quat::from_rotation_z(-17.3f32.to_radians())));
        assert!((lid_gap(&up, &down) - 34.6).abs() < 0.1);
        assert!(lid_gap(&up, &up).abs() < 1e-3);
    }
}
