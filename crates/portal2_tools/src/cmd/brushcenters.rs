//! Lists every brush entity of a class prefix with its name, centre and
//! size, one per line: `brushcenters <map> [class prefix]` (default
//! `trigger_`). For scripted fly-throughs of a map's triggers.
use glam::Vec3;
use source_formats::bsp::Bsp;

pub fn main() {
    let mut args = crate::args().skip(1);
    let map = args.next().expect("usage: brushcenters <map> [class prefix]");
    let prefix = args.next().unwrap_or_else(|| "trigger_".to_owned());
    let fs = crate::game_fs();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    for (i, e) in bsp.entities.iter().enumerate() {
        if !e.classname().starts_with(&prefix) {
            continue;
        }
        let Some(index) = e.get("model").and_then(|m| m.strip_prefix('*')).and_then(|n| n.parse::<usize>().ok()) else { continue };
        let Some(model) = bsp.models.get(index) else { continue };
        let origin = e.origin();
        let (lo, hi): (Vec3, Vec3) = (model.mins + origin, model.maxs + origin);
        let c = (lo + hi) * 0.5;
        println!(
            "#{i} {} {} {:.0} {:.0} {:.0} size {:.0} {:.0} {:.0} disabled {}",
            e.classname(),
            e.get("targetname").filter(|s| !s.is_empty()).unwrap_or("-"),
            c.x,
            c.y,
            c.z,
            hi.x - lo.x,
            hi.y - lo.y,
            hi.z - lo.z,
            e.get("startdisabled").unwrap_or("0")
        );
    }
}
