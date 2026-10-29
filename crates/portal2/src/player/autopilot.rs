//! `--autopilot <steps>`: scripted input for automated movement tests, plus
//! a periodic log of the player's state.
//!
//! Steps are comma separated: `w1.5` wait, `f2` forward, `b1` back, `l1` /
//! `r1` strafe, `j` jump tap, `d1` crouch, `t90` turn (degrees), `e` use,
//! `@12.5` wait until the map clock reads 12.5 s, `c<text>` run console
//! text (for replaying a timed run of the original game).

use bevy::prelude::*;

use super::{Player, PlayerInput, PlayerMoveSet};
use crate::args::Args;

pub struct AutopilotPlugin;

impl Plugin for AutopilotPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AutoFire>()
            .init_resource::<AutoUse>()
            .add_systems(Startup, setup)
            .add_systems(FixedUpdate, drive.before(PlayerMoveSet))
            .add_systems(FixedUpdate, log_state.after(PlayerMoveSet));
    }
}

#[derive(Clone, Debug)]
enum Step {
    Wait(f32),
    Forward(f32),
    Back(f32),
    Left(f32),
    Right(f32),
    Jump,
    Duck(f32),
    Turn(f32),
    Pitch(f32),
    Fire(usize),
    LookAt(Vec3),
    Use,
    Until(f64),
    Command(String),
}

#[derive(Resource)]
struct Autopilot {
    steps: Vec<Step>,
    index: usize,
    elapsed: f32,
    log_timer: f32,
}

fn parse(spec: &str) -> Vec<Step> {
    spec.split(',')
        .filter_map(|s| {
            let s = s.trim();
            if let Some(text) = s.strip_prefix('c') {
                return Some(Step::Command(text.to_owned()));
            }
            if let Some(t) = s.strip_prefix('@') {
                return t.parse().ok().map(Step::Until);
            }
            let (kind, rest) = s.split_at(1.min(s.len()));
            let n: f32 = rest.parse().unwrap_or(1.0);
            Some(match kind {
                "w" => Step::Wait(n),
                "f" => Step::Forward(n),
                "b" => Step::Back(n),
                "l" => Step::Left(n),
                "r" => Step::Right(n),
                "j" => Step::Jump,
                "d" => Step::Duck(n),
                "t" => Step::Turn(n),
                "p" => Step::Pitch(if rest.is_empty() { 0.0 } else { n }),
                "a" => {
                    let v: Vec<f32> = rest.split(';').filter_map(|x| x.parse().ok()).collect();
                    if v.len() != 3 {
                        return None;
                    }
                    Step::LookAt(Vec3::new(v[0], v[1], v[2]))
                }
                "e" => Step::Use,
                "1" => Step::Fire(0),
                "2" => Step::Fire(1),
                _ => return None,
            })
        })
        .collect()
}

fn setup(mut commands: Commands, args: Res<Args>) {
    if let Some(spec) = &args.autopilot {
        commands.insert_resource(Autopilot { steps: parse(spec), index: 0, elapsed: 0.0, log_timer: 0.0 });
    }
}

/// Portal shot requested by the autopilot (consumed by the gun).
#[derive(Resource, Default)]
pub struct AutoFire(pub Option<usize>);

/// "Use" requested by the autopilot (consumed by the carry system).
#[derive(Resource, Default)]
pub struct AutoUse(pub bool);

#[allow(clippy::too_many_arguments)]
fn drive(
    time: Res<Time<Fixed>>,
    clock: Res<crate::clock::GameClock>,
    mut console: ResMut<crate::console::Console>,
    ap: Option<ResMut<Autopilot>>,
    mut input: ResMut<PlayerInput>,
    mut fire: ResMut<AutoFire>,
    mut use_key: ResMut<AutoUse>,
    players: Query<&Player>,
) {
    let Some(mut ap) = ap else { return };
    let dt = time.delta_secs();
    let i = &mut input.input;
    i.forward = 0.0;
    i.right = 0.0;
    i.jump = false;
    i.duck = false;
    let Some(step) = ap.steps.get(ap.index).cloned() else { return };
    let duration = match step {
        Step::Wait(d) | Step::Forward(d) | Step::Back(d) | Step::Left(d) | Step::Right(d) | Step::Duck(d) => d,
        Step::Jump => 2.0 / 60.0,
        Step::Until(t) if clock.curtime < t => return,
        Step::Turn(_) | Step::Pitch(_) | Step::Fire(_) | Step::LookAt(_) | Step::Use | Step::Until(_) | Step::Command(_) => 0.0,
    };
    match step {
        Step::Forward(_) => i.forward = 1.0,
        Step::Back(_) => i.forward = -1.0,
        Step::Left(_) => i.right = -1.0,
        Step::Right(_) => i.right = 1.0,
        Step::Jump => i.jump = true,
        Step::Duck(_) => i.duck = true,
        Step::Turn(deg) => i.angles.y = (i.angles.y + deg).rem_euclid(360.0),
        Step::Pitch(deg) => i.angles.x = deg.clamp(-89.0, 89.0),
        Step::Fire(slot) => fire.0 = Some(slot),
        Step::Use => use_key.0 = true,
        Step::LookAt(target) => {
            if let Ok(p) = players.single() {
                let eye = p.state.origin + Vec3::Z * p.state.eye_offset();
                let d = (target - eye).normalize_or(Vec3::X);
                i.angles.y = d.y.atan2(d.x).to_degrees().rem_euclid(360.0);
                i.angles.x = (-d.z).asin().to_degrees().clamp(-89.0, 89.0);
            }
        }
        Step::Command(ref text) => console.add_trusted_text(text),
        Step::Wait(_) | Step::Until(_) => {}
    }
    ap.elapsed += dt;
    if ap.elapsed >= duration {
        ap.elapsed = 0.0;
        ap.index += 1;
        info!("autopilot step {} {:?} at {:.2}", ap.index, step, clock.curtime);
    }
}

fn log_state(
    time: Res<Time<Fixed>>,
    ap: Option<ResMut<Autopilot>>,
    players: Query<(Entity, &Player)>,
    input: Res<PlayerInput>,
    physics: crate::collision::Physics,
    entities: Query<&crate::entities::MapEntity>,
    props: Query<(&crate::entities::MapEntity, &GlobalTransform), With<crate::entities::props::PhysicsProp>>,
    holding: Option<Res<crate::physics::carry::Holding>>,
) {
    let Some(mut ap) = ap else { return };
    ap.log_timer += time.delta_secs();
    if ap.log_timer < 0.25 {
        return;
    }
    ap.log_timer = 0.0;
    if let Ok((pe, p)) = players.single() {
        let s = &p.state;
        // What is directly ahead of the player (for diagnosing blockages).
        let yaw = input.input.angles.y.to_radians();
        let ahead = Vec3::new(yaw.cos(), yaw.sin(), 0.0) * 8.0;
        let (mins, maxs) = s.hull();
        let up = Vec3::Z * 2.0;
        let tr = physics.trace(s.origin + up, s.origin + up + ahead, mins, maxs, crate::collision::mask::PLAYER_SOLID, &[pe]);
        let blocker = if tr.fraction < 1.0 {
            match tr.entity {
                Some(e) => entities.get(e).map(|m| m.classname.clone()).unwrap_or_else(|_| "entity".into()),
                None => "world".into(),
            }
        } else {
            "-".into()
        };
        let describe = |e: Option<Entity>| match e {
            Some(e) => entities
                .get(e)
                .map(|m| format!("{} '{}'", m.classname, m.targetname.as_deref().unwrap_or("")))
                .unwrap_or_else(|_| "entity".into()),
            None => "world".into(),
        };
        let inside: Vec<String> =
            physics.overlapping(s.origin, mins, maxs, crate::collision::mask::PLAYER_SOLID).into_iter().map(describe).collect();
        info!(
            "player at ({:.1}, {:.1}, {:.1}) speed {:.1} vz {:.1} ground {} on {} ducked {} ahead {} inside [{}]",
            s.origin.x,
            s.origin.y,
            s.origin.z,
            s.velocity.truncate().length(),
            s.velocity.z,
            s.on_ground,
            s.ground_entity.map(|g| describe(Some(g))).unwrap_or_else(|| "-".into()),
            s.ducked,
            blocker,
            inside.join(", ")
        );
        let mut cubes: Vec<String> = props
            .iter()
            .filter(|(m, _)| m.classname == "prop_weighted_cube")
            .map(|(_, t)| {
                let p = t.translation();
                format!("({:.0}, {:.0}, {:.0})", p.x, p.y, p.z)
            })
            .collect();
        cubes.truncate(3);
        info!("  cubes {} holding {}", cubes.join(" "), holding.and_then(|h| h.entity).is_some());
    }
}
