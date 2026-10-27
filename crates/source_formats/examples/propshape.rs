//! Compares a map entity class's collision bounds with its model bounds.
//! Usage: propshape <map> <classname>. Prints numbers only.
use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;
use source_formats::mdl::Mdl;
use source_formats::phy::Phy;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{}.bsp", a[0])).unwrap()).unwrap();
    let Some(ent) = bsp.entities.iter().find(|e| e.classname() == a[1]) else { println!("no entity"); return };
    let Some(model) = ent.get("model") else { println!("no model key"); return };
    let mdl = Mdl::parse(fs.read(model).unwrap()).unwrap();
    println!("model hull {:?} .. {:?}; view bbox {:?} .. {:?}", mdl.hull_min, mdl.hull_max, mdl.view_bbmin, mdl.view_bbmax);
    let base = model.trim_end_matches(".mdl");
    let phy = Phy::parse(&fs.read(&format!("{base}.phy")).unwrap()).unwrap();
    for (i, s) in phy.solids.iter().enumerate() {
        let mut lo = glam::Vec3::splat(f32::MAX);
        let mut hi = glam::Vec3::splat(f32::MIN);
        for p in s.pieces.iter().flat_map(|p| p.points.iter()) { lo = lo.min(*p); hi = hi.max(*p); }
        println!("solid {i}: {} pieces, bounds {lo:.1?} .. {hi:.1?}", s.pieces.len());
    }
    println!("bones {} sequences {}", mdl.bones.len(), mdl.sequences.len());
    for (i, b) in mdl.bones.iter().enumerate().take(4) {
        println!("bone {i}: parent {} pos {:.1?} quat {:.3?}", b.parent, b.pos, b.quat);
    }
}
