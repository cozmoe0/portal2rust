//! Prints the lighting inputs a dynamic model at a point would see: the leaf
//! ambient cube and the strongest PVS-visible world lights with their
//! attenuation terms. Usage: `modellights <map> <x,y,z>` (no point: a census
//! of the map's world lights by type).
use glam::Vec3;
use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;

fn main() {
    let mut args = std::env::args().skip(1);
    let map = args.next().unwrap_or_else(|| "sp_a2_laser_intro".to_owned());
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let kind_name = |k: i32| match k {
        0 => "surface",
        1 => "point",
        2 => "spot",
        3 => "skylight",
        4 => "quakelight",
        5 => "skyambient",
        _ => "?",
    };
    let Some(point) = args.next() else {
        let mut census = std::collections::BTreeMap::new();
        for l in &bsp.world_lights {
            let zero_attn = (l.constant_attn, l.linear_attn, l.quadratic_attn).eq(&(0.0, 0.0, 0.0));
            *census.entry((l.kind, zero_attn, l.flags)).or_insert(0) += 1;
        }
        for ((kind, zero_attn, flags), n) in census {
            println!("{:>10} zero attenuation {zero_attn} flags {flags}: {n}", kind_name(kind));
        }
        for l in bsp.world_lights.iter().filter(|l| l.kind == 0).take(5) {
            println!("surface light: {l:?}");
        }
        return;
    };
    let v: Vec<f32> = point.split(',').map(|s| s.trim().parse().unwrap()).collect();
    let p = Vec3::new(v[0], v[1], v[2]);
    let leaf = bsp.find_leaf(p);
    println!("leaf {leaf} cluster {}", bsp.leafs[leaf].cluster);
    println!("ambient cube {:?}", bsp.ambient_cube(p));
    let pvs = bsp.visibility.as_ref().and_then(|vis| {
        let c = bsp.leafs[leaf].cluster;
        (c >= 0).then(|| vis.pvs(c as usize))
    });
    let mut rows: Vec<(f32, f32, &source_formats::bsp::WorldLight)> = bsp
        .world_lights
        .iter()
        .filter(|l| {
            l.cluster < 0 || pvs.as_ref().is_none_or(|v| v[(l.cluster / 8) as usize] & (1 << (l.cluster % 8)) != 0)
        })
        .map(|l| {
            let dist = (l.origin - p).length().max(1.0);
            let denom = l.constant_attn + l.linear_attn * dist + l.quadratic_attn * dist * dist;
            let i = l.intensity;
            let lum = i.x * 0.2125 + i.y * 0.7154 + i.z * 0.0721;
            (lum / denom.max(1e-4), dist, l)
        })
        .collect();
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (s, dist, l) in rows.iter().take(12) {
        println!(
            "{:>9} strength {s:10.4} dist {dist:7.1} intensity {:?} attn c{} l{} q{} radius {} stopdot {} {} exp {} normal {:?}",
            kind_name(l.kind),
            l.intensity,
            l.constant_attn,
            l.linear_attn,
            l.quadratic_attn,
            l.radius,
            l.stopdot,
            l.stopdot2,
            l.exponent,
            l.normal
        );
    }
}
