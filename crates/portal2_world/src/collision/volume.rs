//! Convex volumes bounded by planes, and swept axis-aligned box tests
//! against them (the Minkowski-sum plane clipping used by BSP engines).

use bevy::math::Vec3;

/// Traces stop this far short of surfaces so the next move starts outside.
pub const DIST_EPSILON: f32 = 0.031_25;

/// A move that ends still in front of a plane, closing on it by less than
/// this, runs along it rather than into it. A box resting within
/// `DIST_EPSILON` of a face that is a hair off level (a prop's collision
/// placed by its animated bones) would otherwise be stopped dead at the
/// start of every move across it: the player stuck on `sp_a1_wakeup`'s
/// opened entry door.
const PARALLEL_SLOP: f32 = 0.001;

#[derive(Clone, Copy, Debug)]
pub struct Plane {
    /// Outward normal; points `p` with `normal·p > dist` are outside.
    pub normal: Vec3,
    pub dist: f32,
    /// Texinfo / surface index for material lookups (-1 = none).
    pub surface: i32,
}

#[derive(Clone, Debug)]
pub struct ConvexVolume {
    pub planes: Vec<Plane>,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub contents: u32,
    /// Corners of the volume (a hull's points; for a volume made from
    /// planes, their intersections), so physics bodies can be built from
    /// them without solving for them again.
    pub points: Vec<Vec3>,
}

/// Result of sweeping a box against one volume.
#[derive(Clone, Copy, Debug)]
pub struct VolumeHit {
    pub fraction: f32,
    pub plane: Plane,
    pub start_solid: bool,
    pub all_solid: bool,
}

impl ConvexVolume {
    /// Builds a volume from planes, computing bounds from axial planes when
    /// present and otherwise from plane intersections.
    pub fn from_planes(planes: Vec<Plane>, contents: u32) -> Option<Self> {
        if planes.len() < 4 {
            return None;
        }
        let mut mins = Vec3::splat(f32::MAX);
        let mut maxs = Vec3::splat(f32::MIN);
        let verts = polytope_vertices(&planes);
        if verts.is_empty() {
            return None;
        }
        for v in &verts {
            mins = mins.min(*v);
            maxs = maxs.max(*v);
        }
        Some(Self { planes, mins, maxs, contents, points: verts })
    }

    /// Builds a volume from a convex point set's triangles, adding axial bevel
    /// planes so box sweeps do not catch on edges.
    pub fn from_hull(points: &[Vec3], triangles: &[[u16; 3]], contents: u32, surface: i32) -> Option<Self> {
        if points.len() < 4 {
            return None;
        }
        let centroid = points.iter().fold(Vec3::ZERO, |a, &b| a + b) / points.len() as f32;
        let mut planes: Vec<Plane> = Vec::with_capacity(triangles.len() + 6);
        for t in triangles {
            let (a, b, c) = (points[t[0] as usize], points[t[1] as usize], points[t[2] as usize]);
            let mut n = (b - a).cross(c - a);
            if n.length_squared() < 1e-8 {
                continue;
            }
            n = n.normalize();
            if n.dot(centroid - a) > 0.0 {
                n = -n;
            }
            // Use the support distance so slightly non-planar hulls stay convex.
            let dist = points.iter().map(|p| n.dot(*p)).fold(f32::MIN, f32::max);
            if planes.iter().any(|p| p.normal.dot(n) > 0.9999 && (p.dist - dist).abs() < 0.01) {
                continue;
            }
            planes.push(Plane { normal: n, dist, surface });
        }
        let mut mins = Vec3::splat(f32::MAX);
        let mut maxs = Vec3::splat(f32::MIN);
        for p in points {
            mins = mins.min(*p);
            maxs = maxs.max(*p);
        }
        add_axial_bevels(&mut planes, mins, maxs, surface);
        (planes.len() >= 4).then_some(Self { planes, mins, maxs, contents, points: points.to_vec() })
    }

    /// Exact volume for a single triangle (zero thickness): the separating
    /// axes of box-vs-triangle give every bounding plane of the Minkowski sum.
    pub fn from_triangle(tri: [Vec3; 3], contents: u32, surface: i32) -> Option<Self> {
        let n = (tri[1] - tri[0]).cross(tri[2] - tri[0]);
        if n.length_squared() < 1e-6 {
            return None;
        }
        let mut axes = vec![n.normalize(), Vec3::X, Vec3::Y, Vec3::Z];
        for e in [tri[1] - tri[0], tri[2] - tri[1], tri[0] - tri[2]] {
            for a in [Vec3::X, Vec3::Y, Vec3::Z] {
                let c = e.cross(a);
                if c.length_squared() > 1e-6 {
                    axes.push(c.normalize());
                }
            }
        }
        let mut planes = Vec::with_capacity(axes.len() * 2);
        for a in axes {
            for dir in [a, -a] {
                let dist = tri.iter().map(|p| dir.dot(*p)).fold(f32::MIN, f32::max);
                if planes.iter().any(|p: &Plane| p.normal.dot(dir) > 0.9999 && (p.dist - dist).abs() < 0.01) {
                    continue;
                }
                planes.push(Plane { normal: dir, dist, surface });
            }
        }
        let mins = tri[0].min(tri[1]).min(tri[2]);
        let maxs = tri[0].max(tri[1]).max(tri[2]);
        Some(Self { planes, mins, maxs, contents, points: tri.to_vec() })
    }

    /// Axis-aligned box volume.
    pub fn from_box(mins: Vec3, maxs: Vec3, contents: u32, surface: i32) -> Self {
        let mut planes = Vec::with_capacity(6);
        add_axial_bevels(&mut planes, mins, maxs, surface);
        let points = (0..8).map(|i| Vec3::select(bevy::math::BVec3::new(i & 1 != 0, i & 2 != 0, i & 4 != 0), maxs, mins)).collect();
        Self { planes, mins, maxs, contents, points }
    }

    /// Sweeps the box `[mins, maxs]` from `start` to `end`.
    pub fn sweep(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3) -> Option<VolumeHit> {
        let mut enter = -1.0f32;
        let mut leave = 1.0f32;
        let mut hit_plane: Option<Plane> = None;
        let mut starts_out = false;
        let mut ends_out = false;
        for p in &self.planes {
            let n = p.normal;
            // Box corner that touches the plane first.
            let corner = Vec3::new(
                if n.x < 0.0 { maxs.x } else { mins.x },
                if n.y < 0.0 { maxs.y } else { mins.y },
                if n.z < 0.0 { maxs.z } else { mins.z },
            );
            let dist = p.dist - n.dot(corner);
            let d1 = n.dot(start) - dist;
            let d2 = n.dot(end) - dist;
            if d1 > 0.0 {
                starts_out = true;
            }
            if d2 > 0.0 {
                ends_out = true;
            }
            // Completely in front of this face: no contact with the volume.
            if d1 > 0.0 && (d2 >= DIST_EPSILON || d2 >= d1 || (d2 > 0.0 && d1 - d2 < PARALLEL_SLOP)) {
                return None;
            }
            if d1 <= 0.0 && d2 <= 0.0 {
                continue;
            }
            if d1 > d2 {
                let f = (d1 - DIST_EPSILON).max(0.0) / (d1 - d2);
                if f > enter {
                    enter = f;
                    hit_plane = Some(*p);
                }
            } else {
                let f = (d1 + DIST_EPSILON) / (d1 - d2);
                if f < leave {
                    leave = f;
                }
            }
        }
        if !starts_out {
            let plane = hit_plane.unwrap_or(Plane { normal: Vec3::Z, dist: 0.0, surface: -1 });
            return Some(VolumeHit { fraction: 0.0, plane, start_solid: true, all_solid: !ends_out });
        }
        if enter < leave
            && enter > -1.0
            && let Some(plane) = hit_plane
        {
            return Some(VolumeHit { fraction: enter.max(0.0), plane, start_solid: false, all_solid: false });
        }
        None
    }

    /// Whether the box at `pos` overlaps the volume.
    pub fn overlaps_box(&self, pos: Vec3, mins: Vec3, maxs: Vec3) -> bool {
        let (bmin, bmax) = (pos + mins, pos + maxs);
        if bmin.cmpgt(self.maxs).any() || bmax.cmplt(self.mins).any() {
            return false;
        }
        self.planes.iter().all(|p| {
            let n = p.normal;
            let corner = Vec3::new(
                if n.x < 0.0 { maxs.x } else { mins.x },
                if n.y < 0.0 { maxs.y } else { mins.y },
                if n.z < 0.0 { maxs.z } else { mins.z },
            );
            n.dot(pos + corner) < p.dist
        })
    }
}

fn add_axial_bevels(planes: &mut Vec<Plane>, mins: Vec3, maxs: Vec3, surface: i32) {
    for axis in 0..3 {
        let mut n = Vec3::ZERO;
        n[axis] = 1.0;
        if !planes.iter().any(|p| p.normal.dot(n) > 0.9999) {
            planes.push(Plane { normal: n, dist: maxs[axis], surface });
        }
        if !planes.iter().any(|p| p.normal.dot(-n) > 0.9999) {
            planes.push(Plane { normal: -n, dist: -mins[axis], surface });
        }
    }
}

/// Vertices of the convex region bounded by `planes` (intersections of
/// plane triples that lie inside every other plane).
pub fn polytope_vertices(planes: &[Plane]) -> Vec<Vec3> {
    let mut out = Vec::new();
    let n = planes.len();
    for i in 0..n {
        for j in i + 1..n {
            for k in j + 1..n {
                let (a, b, c) = (planes[i], planes[j], planes[k]);
                let det = a.normal.dot(b.normal.cross(c.normal));
                if det.abs() < 1e-6 {
                    continue;
                }
                let p = (b.normal.cross(c.normal) * a.dist + c.normal.cross(a.normal) * b.dist + a.normal.cross(b.normal) * c.dist) / det;
                if planes.iter().all(|q| q.normal.dot(p) <= q.dist + 0.05) {
                    out.push(p);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_sweep_hits_face() {
        let v = ConvexVolume::from_box(Vec3::new(0.0, -10.0, -10.0), Vec3::new(10.0, 10.0, 10.0), 1, -1);
        let hit = v.sweep(Vec3::new(-20.0, 0.0, 0.0), Vec3::new(20.0, 0.0, 0.0), Vec3::splat(-1.0), Vec3::splat(1.0)).unwrap();
        // Box face at x = -1 reaches x = 0 after 19 of 40 units.
        assert!((hit.fraction - 19.0 / 40.0).abs() < 0.01);
        assert!(hit.plane.normal.abs_diff_eq(-Vec3::X, 1e-5));
        assert!(!hit.start_solid);
    }

    #[test]
    fn slide_along_slightly_tilted_face() {
        // A floor whose top leans by a few millionths, with the box resting
        // just inside the keep-off distance above it.
        let n = Vec3::new(0.0, -5e-6, 1.0).normalize();
        let mut v = ConvexVolume::from_box(Vec3::splat(-100.0), Vec3::new(100.0, 100.0, 0.0), 1, -1);
        for p in &mut v.planes {
            if p.normal.z > 0.5 {
                p.normal = n;
            }
        }
        let start = Vec3::new(0.0, 0.0, 0.02);
        let hit = v.sweep(start, start + Vec3::new(0.0, 3.0, 0.0), Vec3::new(-16.0, -16.0, 0.0), Vec3::new(16.0, 16.0, 72.0));
        assert!(hit.is_none());
        // Moving down onto it still lands.
        let hit = v.sweep(Vec3::new(0.0, 0.0, 10.0), Vec3::new(0.0, 0.0, -10.0), Vec3::new(-16.0, -16.0, 0.0), Vec3::new(16.0, 16.0, 72.0));
        assert!(hit.is_some_and(|h| !h.start_solid && h.fraction > 0.4));
    }

    #[test]
    fn start_solid_detected() {
        let v = ConvexVolume::from_box(Vec3::splat(-10.0), Vec3::splat(10.0), 1, -1);
        let hit = v.sweep(Vec3::ZERO, Vec3::new(0.0, 0.0, 5.0), Vec3::splat(-1.0), Vec3::splat(1.0)).unwrap();
        assert!(hit.start_solid && hit.all_solid);
    }

    #[test]
    fn triangle_volume_blocks() {
        let tri = [Vec3::new(-50.0, -50.0, 0.0), Vec3::new(50.0, -50.0, 0.0), Vec3::new(0.0, 50.0, 0.0)];
        let v = ConvexVolume::from_triangle(tri, 1, -1).unwrap();
        let hit = v.sweep(Vec3::new(0.0, 0.0, 20.0), Vec3::new(0.0, 0.0, -20.0), Vec3::new(-4.0, -4.0, 0.0), Vec3::new(4.0, 4.0, 8.0));
        let hit = hit.unwrap();
        assert!((hit.fraction - 0.5).abs() < 0.01);
    }
}
