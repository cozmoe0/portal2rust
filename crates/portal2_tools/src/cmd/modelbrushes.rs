//! Lists the brushes of a brush model with their contents and axial bounds:
//! `modelbrushes <map> <model index>`.
use std::collections::HashSet;

use glam::Vec3;
use source_formats::bsp::Bsp;

pub fn main() {
    let mut args = crate::args().skip(1);
    let map = args.next().expect("usage: modelbrushes <map> <model index>");
    let index: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let fs = crate::game_fs();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let m = &bsp.models[index];
    let mut seen = HashSet::new();
    let mut stack = vec![m.head_node];
    while let Some(n) = stack.pop() {
        if n >= 0 {
            let node = &bsp.nodes[n as usize];
            stack.extend(node.children);
            continue;
        }
        let leaf = &bsp.leafs[(-1 - n) as usize];
        for i in leaf.first_leaf_brush..leaf.first_leaf_brush + leaf.num_leaf_brushes {
            let b = bsp.leaf_brushes[i as usize] as usize;
            if !seen.insert(b) {
                continue;
            }
            let brush = &bsp.brushes[b];
            let (mut mins, mut maxs) = (Vec3::splat(f32::MIN), Vec3::splat(f32::MAX));
            for s in brush.first_side..brush.first_side + brush.num_sides {
                let p = &bsp.planes[bsp.brush_sides[s as usize].plane as usize];
                for axis in 0..3 {
                    if (p.normal[axis] - 1.0).abs() < 1e-4 {
                        maxs[axis] = maxs[axis].min(p.dist);
                    } else if (p.normal[axis] + 1.0).abs() < 1e-4 {
                        mins[axis] = mins[axis].max(-p.dist);
                    }
                }
            }
            println!("brush {b}: contents {:#x} sides {} {mins:?} .. {maxs:?}", brush.contents, brush.num_sides);
            if std::env::var_os("P2T_SIDES").is_some() {
                for s in brush.first_side..brush.first_side + brush.num_sides {
                    let side = &bsp.brush_sides[s as usize];
                    let p = &bsp.planes[side.plane as usize];
                    println!("  plane {:?} dist {} bevel {}", p.normal, p.dist, side.bevel);
                }
            }
        }
    }
}
