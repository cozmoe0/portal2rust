//! Builds collision volumes from map data: brushes per brush model,
//! displacement triangles, and static props (model hulls or boxes).

use std::collections::HashSet;

use bevy::math::{Mat4, Quat, Vec3};
use source_formats::bsp::{Bsp, contents};
use source_formats::fs::GameFileSystem;
use source_formats::mdl::Mdl;
use source_formats::phy::Phy;
use source_formats::text::strip_extension;

use super::volume::{ConvexVolume, Plane};
use portal2_core::coords::angles_to_quat;

/// Brush indices reachable from a model's head node.
pub fn model_brushes(bsp: &Bsp, model: usize) -> Vec<usize> {
    let Some(m) = bsp.models.get(model) else { return Vec::new() };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut stack = vec![m.head_node];
    while let Some(n) = stack.pop() {
        if n < 0 {
            let leaf = (-1 - n) as usize;
            let Some(l) = bsp.leafs.get(leaf) else { continue };
            for i in l.first_leaf_brush..l.first_leaf_brush + l.num_leaf_brushes {
                let b = bsp.leaf_brushes[i as usize] as usize;
                if seen.insert(b) {
                    out.push(b);
                }
            }
        } else if let Some(node) = bsp.nodes.get(n as usize) {
            stack.push(node.children[0]);
            stack.push(node.children[1]);
        }
    }
    out
}

pub fn brush_volume(bsp: &Bsp, brush: usize) -> Option<ConvexVolume> {
    let b = bsp.brushes.get(brush)?;
    let mut planes = Vec::with_capacity(b.num_sides.max(0) as usize);
    for s in b.first_side..b.first_side + b.num_sides {
        let side = bsp.brush_sides.get(s as usize)?;
        let p = bsp.planes.get(side.plane as usize)?;
        planes.push(Plane { normal: p.normal, dist: p.dist, surface: side.texinfo as i32 });
    }
    ConvexVolume::from_planes(planes, b.contents)
}

/// Volumes of every brush of a brush model (model space).
pub fn model_volumes(bsp: &Bsp, model: usize) -> Vec<ConvexVolume> {
    model_brushes(bsp, model).into_iter().filter_map(|b| brush_volume(bsp, b)).collect()
}

/// The map's physics collision (`LUMP_PHYSCOLLIDE`) for every brush model,
/// indexed by model: the convex pieces the game's physics builds from the
/// brushes, each pulled in by half a unit. The engine traces brush entities
/// that are physics-solid (doors, trains, `func_brush`...) through these
/// rather than through the brushes. Each piece takes its contents from the
/// brush it came from and each face the surface of that brush's face
/// pointing the same way, so material lookups still work.
pub fn model_physics(bsp: &Bsp) -> Vec<Option<Vec<ConvexVolume>>> {
    let mut out: Vec<Option<Vec<ConvexVolume>>> = vec![None; bsp.models.len()];
    let data = bsp.phys_collide.as_slice();
    let read = |at: usize| data.get(at..at + 4).map(|b| i32::from_le_bytes(b.try_into().expect("four bytes")));
    let mut at = 0;
    while let (Some(model), Some(size), Some(key_size), Some(solids)) = (read(at), read(at + 4), read(at + 8), read(at + 12)) {
        if model < 0 || size < 0 || key_size < 0 {
            break;
        }
        let start = at + 16;
        at = start + size as usize + key_size as usize;
        let Some(slot) = out.get_mut(model as usize) else { continue };
        if model == 0 {
            continue;
        }
        let brushes = model_volumes(bsp, model as usize);
        let mut pieces = Vec::new();
        let mut cursor = start;
        for _ in 0..solids.max(0) {
            let Some(len) = read(cursor).filter(|l| *l >= 0).map(|l| l as usize) else { break };
            let Some(block) = data.get(cursor + 4..cursor + 4 + len) else { break };
            cursor += 4 + len;
            let Some(solid) = Phy::parse_surface(block) else { continue };
            for p in &solid.pieces {
                let centre = p.points.iter().copied().sum::<Vec3>() / p.points.len().max(1) as f32;
                let source = brushes.iter().find(|b| (b.mins - Vec3::ONE).cmple(centre).all() && centre.cmple(b.maxs + Vec3::ONE).all());
                let contents = source.map_or(contents::SOLID, |b| b.contents);
                let Some(mut v) = ConvexVolume::from_hull(&p.points, &p.triangles, contents, -1) else { continue };
                if let Some(b) = source {
                    for plane in &mut v.planes {
                        let best = b.planes.iter().max_by(|x, y| x.normal.dot(plane.normal).total_cmp(&y.normal.dot(plane.normal)));
                        if let Some(best) = best.filter(|q| q.normal.dot(plane.normal) > 0.9) {
                            plane.surface = best.surface;
                        }
                    }
                }
                pieces.push(v);
            }
        }
        if !pieces.is_empty() {
            *slot = Some(pieces);
        }
    }
    out
}

/// Every displacement (terrain) triangle in world space.
pub fn displacement_triangles(bsp: &Bsp) -> Vec<[Vec3; 3]> {
    let mut out = Vec::new();
    for disp in &bsp.disp_infos {
        let Some(face) = bsp.faces.get(disp.map_face as usize) else { continue };
        let corners = bsp.face_vertices(face);
        if corners.len() != 4 {
            continue;
        }
        let start = (0..4)
            .min_by(|&a, &b| corners[a].distance_squared(disp.start_position).total_cmp(&corners[b].distance_squared(disp.start_position)))
            .unwrap_or(0);
        let c = [corners[start], corners[(start + 1) % 4], corners[(start + 2) % 4], corners[(start + 3) % 4]];
        let n = disp.side_verts();
        let denom = (n - 1) as f32;
        let mut pos = Vec::with_capacity(n * n);
        for i in 0..n {
            let fi = i as f32 / denom;
            let a = c[0].lerp(c[1], fi);
            let b = c[3].lerp(c[2], fi);
            for j in 0..n {
                let p = a.lerp(b, j as f32 / denom);
                let dv = &bsp.disp_verts[disp.disp_vert_start as usize + i * n + j];
                pos.push(p + dv.vec * dv.dist);
            }
        }
        for i in 0..n - 1 {
            for j in 0..n - 1 {
                let v00 = pos[i * n + j];
                let v01 = pos[i * n + j + 1];
                let v10 = pos[(i + 1) * n + j];
                let v11 = pos[(i + 1) * n + j + 1];
                if (i + j) % 2 == 0 {
                    out.push([v00, v10, v11]);
                    out.push([v00, v11, v01]);
                } else {
                    out.push([v00, v10, v01]);
                    out.push([v01, v10, v11]);
                }
            }
        }
    }
    out
}

/// One volume per displacement triangle (world space). Terrain never takes
/// portals, so its surface index is -2.
pub fn displacement_volumes(bsp: &Bsp) -> Vec<ConvexVolume> {
    displacement_triangles(bsp).into_iter().filter_map(|t| ConvexVolume::from_triangle(t, contents::SOLID, -2)).collect()
}

/// `solid` keyvalue: collide with the model's hull box.
pub const SOLID_BBOX: i32 = 2;

/// Collision for a studio model in model space. Physics-solid models
/// (`solid` 6) use their `.phy` hulls and do not collide without one, as in
/// the original; box-solid models (`solid` 2) use the model's hull box.
pub fn studio_volumes(fs: &GameFileSystem, path: &str, solid: i32) -> Option<Vec<ConvexVolume>> {
    if solid == SOLID_BBOX {
        return hull_box(fs, path);
    }
    let base = strip_extension(path);
    if let Some(bytes) = fs.read(&format!("{base}.phy"))
        && let Ok(phy) = Phy::parse(&bytes)
    {
        // Each solid is stored in the space of the bone it is attached to
        // (named in the trailing keyvalues); place it with that bone's
        // default pose.
        let poses = solid_bone_poses(fs, path, &phy);
        let vols: Vec<ConvexVolume> = phy
            .solids
            .iter()
            .enumerate()
            .flat_map(|(i, s)| {
                let m = poses.get(&i).copied().unwrap_or(Mat4::IDENTITY);
                s.pieces.iter().filter_map(move |p| {
                    let pts: Vec<Vec3> = p.points.iter().map(|q| m.transform_point3(*q)).collect();
                    ConvexVolume::from_hull(&pts, &p.triangles, contents::SOLID, -1)
                })
            })
            .collect();
        if !vols.is_empty() {
            return Some(vols);
        }
    }
    None
}

/// Collision that follows a model's bones (the engine's bone followers):
/// the bones listed in the model's `bone_followers` keyvalues, or when it
/// has none and its `.phy` holds more than one solid, every solid on its
/// own bone. Volumes are in the space of their bone. `None` when the model
/// collides as one piece.
pub fn studio_bone_solids(fs: &GameFileSystem, path: &str) -> Option<Vec<(usize, Vec<ConvexVolume>)>> {
    let bytes = fs.read(&format!("{}.phy", strip_extension(path)))?;
    let phy = Phy::parse(&bytes).ok()?;
    let model = source_formats::studio::StudioModel::load(fs, path).ok()?;
    // Bone of each solid, from the collision keyvalues.
    let mut solid_bones: Vec<Option<usize>> = vec![None; phy.solids.len()];
    for r in source_formats::kv::parse_document(&phy.keyvalues).roots {
        if !r.key.eq_ignore_ascii_case("solid") {
            continue;
        }
        if let (Some(index), Some(bone)) = (r.get_i32("index"), r.get_str("name").and_then(|n| model.mdl.find_bone(n)))
            && let Some(slot) = solid_bones.get_mut(index.max(0) as usize)
        {
            *slot = Some(bone);
        }
    }
    // The model's keyvalues sit in an `mdlkeyvalue` block.
    let kv = source_formats::kv::parse_document(&model.mdl.keyvalues);
    let listed: Vec<usize> = kv
        .roots
        .iter()
        .flat_map(|r| std::iter::once(r).chain(r.children()))
        .find(|r| r.key.eq_ignore_ascii_case("bone_followers"))
        .map(|r| r.children().iter().filter_map(|c| c.as_str().and_then(|n| model.mdl.find_bone(n))).collect())
        .unwrap_or_default();
    if listed.is_empty() && phy.solids.len() < 2 {
        return None;
    }
    let out: Vec<(usize, Vec<ConvexVolume>)> = phy
        .solids
        .iter()
        .zip(&solid_bones)
        .filter_map(|(s, bone)| {
            let bone = (*bone)?;
            if !listed.is_empty() && !listed.contains(&bone) {
                return None;
            }
            let vols: Vec<ConvexVolume> =
                s.pieces.iter().filter_map(|p| ConvexVolume::from_hull(&p.points, &p.triangles, contents::SOLID, -1)).collect();
            (!vols.is_empty()).then_some((bone, vols))
        })
        .collect();
    (!out.is_empty()).then_some(out)
}

fn hull_box(fs: &GameFileSystem, path: &str) -> Option<Vec<ConvexVolume>> {
    let mdl = Mdl::parse(fs.read(path)?).ok()?;
    let (mins, maxs) = (mdl.hull_min, mdl.hull_max);
    if (maxs - mins).min_element() <= 0.0 {
        return None;
    }
    Some(vec![ConvexVolume::from_box(mins, maxs, contents::SOLID, -1)])
}

/// Default-pose transform of the bone each collision solid belongs to.
fn solid_bone_poses(fs: &GameFileSystem, path: &str, phy: &Phy) -> std::collections::HashMap<usize, Mat4> {
    let mut out = std::collections::HashMap::new();
    let Ok(model) = source_formats::studio::StudioModel::load(fs, path) else { return out };
    let mut local = vec![(Vec3::ZERO, Quat::IDENTITY); model.mdl.bones.len()];
    model.sample_sequence(0, 0.0, &mut local);
    let pose = model.bone_matrices(&local);
    for r in source_formats::kv::parse_document(&phy.keyvalues).roots {
        if !r.key.eq_ignore_ascii_case("solid") {
            continue;
        }
        let (Some(index), Some(bone)) = (r.get_i32("index"), r.get_str("name").and_then(|n| model.mdl.find_bone(n))) else {
            continue;
        };
        if let Some(m) = pose.get(bone) {
            out.insert(index.max(0) as usize, *m);
        }
    }
    out
}

/// Transforms model-space volumes into world space (rotation, translation,
/// uniform scale) for static geometry.
pub fn transform_volumes(vols: &[ConvexVolume], rotation: Quat, translation: Vec3, scale: f32) -> Vec<ConvexVolume> {
    vols.iter()
        .filter_map(|v| {
            let planes: Vec<Plane> = v
                .planes
                .iter()
                .map(|p| {
                    let n = rotation * p.normal;
                    Plane { normal: n, dist: p.dist * scale + n.dot(translation), surface: p.surface }
                })
                .collect();
            let mut vol = ConvexVolume::from_planes(planes, v.contents)?;
            // Rotated hulls lose their axial bevels; restore them from the bounds.
            let (mins, maxs) = (vol.mins, vol.maxs);
            for axis in 0..3 {
                let mut n = Vec3::ZERO;
                n[axis] = 1.0;
                if !vol.planes.iter().any(|p| p.normal.dot(n) > 0.9999) {
                    vol.planes.push(Plane { normal: n, dist: maxs[axis], surface: -1 });
                }
                if !vol.planes.iter().any(|p| p.normal.dot(-n) > 0.9999) {
                    vol.planes.push(Plane { normal: -n, dist: -mins[axis], surface: -1 });
                }
            }
            Some(vol)
        })
        .collect()
}

/// Static prop collision in world space.
pub fn static_prop_volumes(bsp: &Bsp, fs: &GameFileSystem) -> Vec<ConvexVolume> {
    let mut cache: std::collections::HashMap<(u16, i32), Option<Vec<ConvexVolume>>> = std::collections::HashMap::new();
    let mut out = Vec::new();
    for prop in &bsp.static_props.props {
        if prop.solid == 0 {
            continue;
        }
        let solid = prop.solid as i32;
        let local = cache
            .entry((prop.model, solid))
            .or_insert_with(|| bsp.static_props.models.get(prop.model as usize).and_then(|p| studio_volumes(fs, p, solid)));
        let Some(local) = local else { continue };
        out.extend(transform_volumes(local, angles_to_quat(prop.angles), prop.origin, prop.uniform_scale.max(0.001)));
    }
    out
}
