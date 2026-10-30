//! For static props near a point, prints the lighting origin the engine uses
//! (the lump's when flagged, else the model's illumination point placed with
//! the prop) and the cube map sample nearest to it. Usage:
//! `propcube <map> x,y,z [radius]`.
use glam::{Quat, Vec3};
use source_formats::bsp::Bsp;
use source_formats::studio::StudioModel;

/// Source angles (pitch, yaw, roll in degrees) as a rotation.
fn angles_to_quat(a: Vec3) -> Quat {
    Quat::from_rotation_z(a.y.to_radians()) * Quat::from_rotation_y(a.x.to_radians()) * Quat::from_rotation_x(a.z.to_radians())
}

pub fn main() {
    let mut args = crate::args().skip(1);
    let map = args.next().expect("usage: propcube <map> x,y,z [radius]");
    let p: Vec<f32> = args.next().unwrap_or_default().split(',').filter_map(|s| s.parse().ok()).collect();
    let radius: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(64.0);
    let point = Vec3::new(p[0], p[1], p[2]);
    let fs = crate::game_fs();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let nearest = |q: Vec3| {
        bsp.cubemaps
            .iter()
            .map(|c| (Vec3::new(c.origin[0] as f32, c.origin[1] as f32, c.origin[2] as f32), c.origin))
            .min_by(|a, b| a.0.distance_squared(q).total_cmp(&b.0.distance_squared(q)))
            .map(|(v, o)| (o, v.distance(q)))
    };
    for prop in &bsp.static_props.props {
        if prop.origin.distance(point) > radius {
            continue;
        }
        let model = bsp.static_props.models.get(prop.model as usize).cloned().unwrap_or_default();
        let illum = StudioModel::load(&fs, &model).map(|m| m.mdl.illum_position).unwrap_or(Vec3::ZERO);
        let origin = if prop.flags & 2 != 0 { prop.lighting_origin } else { prop.origin + angles_to_quat(prop.angles) * illum };
        println!("{model} at {:?} flags {:#x} lighting origin {origin:?} -> cube {:?}", prop.origin, prop.flags, nearest(origin));
    }
}
