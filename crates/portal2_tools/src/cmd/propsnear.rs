//! Lists static props whose origin lies within a radius of a point:
//! `propsnear <map> x,y,z [radius]` (model paths and placement only).
use source_formats::bsp::Bsp;

pub fn main() {
    let mut args = crate::args().skip(1);
    let map = args.next().unwrap_or_else(|| "sp_a2_laser_intro".to_owned());
    let p: Vec<f32> = args.next().unwrap_or_default().split(',').filter_map(|s| s.parse().ok()).collect();
    let radius: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(128.0);
    let point = glam::Vec3::new(p[0], p[1], p[2]);
    let fs = crate::game_fs();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    for prop in &bsp.static_props.props {
        let d = prop.origin.distance(point);
        if d <= radius {
            let model = bsp.static_props.models.get(prop.model as usize).map(String::as_str).unwrap_or("?");
            println!("{model} at {:?} angles {:?} solid {} dist {d:.0}", prop.origin, prop.angles, prop.solid);
        }
    }
}
