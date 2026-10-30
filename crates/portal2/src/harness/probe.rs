//! `--probe`: after a few frames, reports which rendered objects the camera's
//! view ray passes through (closest first), and where the collision world
//! along the same ray is solid. Text-only geometry debugging.

use bevy::picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings, RayCastVisibility};
use bevy::prelude::*;

use portal2_core::args::Args;
use portal2_core::coords::angles_forward;
use portal2_core::view::{MainCamera, ViewAngles};
use portal2_world::collision::{Physics, mask};

pub struct ProbePlugin;

impl Plugin for ProbePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Last, probe);
    }
}

fn root_name(mut e: Entity, parents: &Query<&ChildOf>, names: &Query<&Name>) -> String {
    while let Ok(p) = parents.get(e) {
        e = p.parent();
    }
    names.get(e).map(|n| n.as_str().split(' ').next().unwrap_or("").to_owned()).unwrap_or_default() + &format!(" #{}", e.index())
}

fn probe(
    args: Res<Args>,
    mut frame: Local<u32>,
    cam: Query<&ViewAngles, With<MainCamera>>,
    mut ray_cast: MeshRayCast,
    parents: Query<&ChildOf>,
    names: Query<&Name>,
    physics: Physics,
) {
    if !args.probe {
        return;
    }
    *frame += 1;
    if *frame != 30 {
        return;
    }
    let Ok(view) = cam.single() else { return };
    let origin = view.origin;
    let dir = angles_forward(view.angles);
    let Ok(dir3) = Dir3::new(dir) else { return };
    let settings = MeshRayCastSettings::default().with_visibility(RayCastVisibility::Any).never_early_exit();
    let hits = ray_cast.cast_ray(Ray3d::new(origin, dir3), &settings);
    info!("probe from {origin:?} dir {dir:?}: {} triangle hits", hits.len());
    for (e, hit) in hits.iter().take(8) {
        info!("  {:8.1} units  {}  at {:?}", hit.distance, root_name(*e, &parents, &names), hit.point);
    }
    // Solid and empty stretches of the collision world along the ray.
    let tr = physics.trace(origin, origin + dir * 4096.0, Vec3::ZERO, Vec3::ZERO, mask::SOLID_ALL, &[]);
    info!(
        "  trace: {:.1} units, contents {:#x}, entity {:?}, surface {}, start solid {}",
        tr.fraction * 4096.0,
        tr.contents,
        tr.entity,
        tr.surface,
        tr.start_solid
    );
    let mut spans = Vec::new();
    let mut last: Option<(bool, f32)> = None;
    for i in 0..=256 {
        let d = i as f32 * 4.0;
        let p = origin + dir * d;
        let solid = physics.trace(p, p, Vec3::ZERO, Vec3::ZERO, mask::SOLID_ALL, &[]).start_solid;
        match last {
            Some((s, _)) if s == solid => {}
            _ => {
                spans.push(format!("{}@{d}", if solid { "solid" } else { "empty" }));
                last = Some((solid, d));
            }
        }
    }
    info!("  spans: {}", spans.join(" "));
}
