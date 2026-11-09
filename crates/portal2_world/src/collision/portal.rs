//! The geometry of a portal as the collision world sees it: the frame a
//! portal sits in, the opening it cuts through its wall (so movement can
//! pass), and the transform that carries positions and views from one
//! portal to the other.

use std::f32::consts::PI;

use bevy::math::Affine3A;
use bevy::prelude::*;

use super::CollisionWorld;
use super::volume::{ConvexVolume, Plane};

/// Half the size of a portal's opening.
pub const HALF_WIDTH: f32 = 32.0;
pub const HALF_HEIGHT: f32 = 56.0;

/// A portal's placement: centre on the surface, outward normal and up axis.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PortalFrame {
    pub center: Vec3,
    pub forward: Vec3,
    pub up: Vec3,
}

impl PortalFrame {
    pub fn left(&self) -> Vec3 {
        self.up.cross(self.forward)
    }

    /// Local axes: X = out of the surface, Y = left, Z = up.
    pub fn rotation(&self) -> Quat {
        Quat::from_mat3(&Mat3::from_cols(self.forward, self.left(), self.up)).normalize()
    }

    pub fn affine(&self) -> Affine3A {
        Affine3A::from_rotation_translation(self.rotation(), self.center)
    }

    pub fn to_local(&self, p: Vec3) -> Vec3 {
        let d = p - self.center;
        Vec3::new(d.dot(self.forward), d.dot(self.left()), d.dot(self.up))
    }

    pub fn to_world(&self, l: Vec3) -> Vec3 {
        self.center + self.forward * l.x + self.left() * l.y + self.up * l.z
    }

    pub fn plane_dist(&self) -> f32 {
        self.forward.dot(self.center)
    }

    /// Whether a point projected onto the portal lies inside the opening.
    pub fn in_opening(&self, local: Vec3, margin: f32) -> bool {
        let y = local.y / (HALF_WIDTH - margin).max(1.0);
        let z = local.z / (HALF_HEIGHT - margin).max(1.0);
        y * y + z * z <= 1.0
    }

    /// Whether an axis-aligned box at `center` with half-size `half`
    /// straddles the portal's plane within reach of its rectangle: the box
    /// is passing through the opening (the game's portal environment).
    pub fn touches_box(&self, center: Vec3, half: Vec3) -> bool {
        let l = self.to_local(center);
        let extent = |axis: Vec3| axis.x.abs() * half.x + axis.y.abs() * half.y + axis.z.abs() * half.z;
        l.x.abs() <= extent(self.forward) && l.y.abs() <= HALF_WIDTH + extent(self.left()) && l.z.abs() <= HALF_HEIGHT + extent(self.up)
    }
}

/// Maps space in front of `from` to space in front of `to` (entering one
/// portal means leaving the other, turned around).
pub fn transfer(from: &PortalFrame, to: &PortalFrame) -> Affine3A {
    to.affine() * Affine3A::from_rotation_z(PI) * from.affine().inverse()
}

/// Openings that let movement pass through the wall a linked portal is on.
#[derive(Resource, Default, Clone)]
pub struct PortalHoles {
    pub holes: Vec<PortalHole>,
}

#[derive(Clone, Debug)]
pub struct PortalHole {
    pub frame: PortalFrame,
    /// Walls around the opening that keep movers inside the tunnel.
    pub frame_volumes: Vec<ConvexVolume>,
    /// Static volumes the opening cuts through (sorted indices into
    /// `CollisionWorld::statics`): whatever reaches into the space behind
    /// the opening, however far the surface sits behind the portal.
    pub carved: Vec<u32>,
}

/// How far behind the portal the opening cuts through the world.
pub const CARVE_DEPTH: f32 = 64.0;

impl PortalHole {
    pub fn new(frame: PortalFrame, world: &CollisionWorld) -> Self {
        // Four slabs around the rectangular opening, reaching 96 units
        // behind the surface.
        let depth = 96.0;
        let t = 48.0;
        let slab = |center_local: Vec3, half: Vec3| -> Option<ConvexVolume> {
            let axes = [frame.forward, frame.left(), frame.up];
            let c = frame.to_world(center_local);
            let mut planes = Vec::with_capacity(6);
            for (i, a) in axes.iter().enumerate() {
                planes.push(Plane { normal: *a, dist: a.dot(c) + half[i], surface: -1 });
                planes.push(Plane { normal: -*a, dist: -a.dot(c) + half[i], surface: -1 });
            }
            ConvexVolume::from_planes(planes, source_formats::bsp::contents::SOLID)
        };
        let (hw, hh) = (HALF_WIDTH, HALF_HEIGHT);
        let frame_volumes = [
            slab(Vec3::new(-depth / 2.0, hw + t, 0.0), Vec3::new(depth / 2.0, t, hh + 2.0 * t)),
            slab(Vec3::new(-depth / 2.0, -hw - t, 0.0), Vec3::new(depth / 2.0, t, hh + 2.0 * t)),
            slab(Vec3::new(-depth / 2.0, 0.0, hh + t), Vec3::new(depth / 2.0, hw, t)),
            slab(Vec3::new(-depth / 2.0, 0.0, -hh - t), Vec3::new(depth / 2.0, hw, t)),
        ]
        .into_iter()
        .flatten()
        .collect();
        Self { frame, frame_volumes, carved: carved_statics(&frame, world) }
    }

    /// Whether the opening cuts away static volume `index`.
    pub fn carves(&self, index: u32) -> bool {
        self.carved.binary_search(&index).is_ok()
    }

    /// Whether the opening cuts away a moving volume with these bounding
    /// corners: one lying wholly behind the portal and reaching into the
    /// space behind the opening.
    pub fn carves_bounds(&self, corners: &[Vec3]) -> bool {
        let f = &self.frame;
        let local: Vec<Vec3> = corners.iter().map(|&c| f.to_local(c)).collect();
        let (lo, hi) = local.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), l| (lo.min(*l), hi.max(*l)));
        hi.x <= 0.5
            && lo.x < 0.0
            && hi.x > -CARVE_DEPTH
            && lo.y < HALF_WIDTH - 1.0
            && hi.y > -(HALF_WIDTH - 1.0)
            && lo.z < HALF_HEIGHT - 1.0
            && hi.z > -(HALF_HEIGHT - 1.0)
    }

    /// Whether a box at `pos` reaches into the opening's tunnel, so the wall
    /// the portal is on should not block it. It need not fit inside the
    /// opening: the frame slabs, reaching 96 units past every edge, hold a
    /// box that overlaps the rim (the player stepping into a portal whose
    /// bottom edge sits a little above the floor, a cube resting half over a
    /// floor portal) out of the wall around it.
    pub fn overlaps_box(&self, pos: Vec3, mins: Vec3, maxs: Vec3) -> bool {
        let c = pos + (mins + maxs) * 0.5;
        let h = (maxs - mins) * 0.5;
        let f = &self.frame;
        let extent = |axis: Vec3| axis.x.abs() * h.x + axis.y.abs() * h.y + axis.z.abs() * h.z;
        let l = f.to_local(c);
        let (ex, ey, ez) = (extent(f.forward), extent(f.left()), extent(f.up));
        l.y.abs() - ey < HALF_WIDTH && l.z.abs() - ez < HALF_HEIGHT && l.x - ex < 4.0 && l.x + ex > -96.0
    }
}

/// Static volumes reaching into the space behind an opening (a little
/// inside its edges, so floors and side walls meeting the rim stay).
pub fn carved_statics(frame: &PortalFrame, world: &CollisionWorld) -> Vec<u32> {
    let (hw, hh) = (HALF_WIDTH - 1.0, HALF_HEIGHT - 1.0);
    let mut corners = Vec::with_capacity(8);
    for x in [-CARVE_DEPTH, 0.0] {
        for y in [-hw, hw] {
            for z in [-hh, hh] {
                corners.push(frame.to_world(Vec3::new(x, y, z)));
            }
        }
    }
    let plane = |normal: Vec3, local_extent: f32| Plane { normal, dist: normal.dot(frame.center) + local_extent, surface: -1 };
    let region = [
        plane(frame.forward, 0.0),
        plane(-frame.forward, CARVE_DEPTH),
        plane(frame.left(), hw),
        plane(-frame.left(), hw),
        plane(frame.up, hh),
        plane(-frame.up, hh),
    ];
    let (mins, maxs) = corners.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), c| (lo.min(*c), hi.max(*c)));
    let mut carved: Vec<u32> = world
        .statics_in(mins, maxs)
        .into_iter()
        .filter(|&i| {
            let v = &world.statics[i as usize];
            // Separated when every corner of one lies outside a face of the other.
            if v.planes.iter().any(|p| corners.iter().all(|c| p.normal.dot(*c) > p.dist + 0.01)) {
                return false;
            }
            let verts = crate::collision::volume::polytope_vertices(&v.planes);
            !region.iter().any(|p| verts.iter().all(|c| p.normal.dot(*c) > p.dist + 0.01))
        })
        .collect();
    carved.sort_unstable();
    carved.dedup();
    carved
}

/// Carries a view through a linked portal when what it shows lies beyond
/// the opening: the whole near plane (half-size `near_half` at distance
/// `near`) is behind the portal inside its opening, or the near plane
/// straddles the portal and the eye itself is inside the wall behind the
/// opening (it went through ahead of the body or is trailing it). A near
/// plane wholly in front of the portal needs nothing, even from an eye just
/// behind it: that view looks back out of the opening. Only a portal whose
/// opening the body (box at `body_center`, half-size `body_half`) is
/// passing through carries the view: an eye behind a thin wall with the
/// body there too sees what is behind the wall, not the other side.
pub fn view_through_portals(
    frames: &[PortalFrame; 2],
    eye: Vec3,
    rot: Quat,
    near: f32,
    near_half: Vec2,
    body_center: Vec3,
    body_half: Vec3,
) -> (Vec3, Quat) {
    let (f, r, u) = (rot * Vec3::X, rot * Vec3::NEG_Y, rot * Vec3::Z);
    let corners = [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
        .map(|(sx, sy)| eye + f * near + r * (near_half.x * sx) + u * (near_half.y * sy));
    for slot in 0..2 {
        let (from, to) = (frames[slot], frames[1 - slot]);
        if !from.touches_box(body_center, body_half) {
            continue;
        }
        let in_tube = |l: Vec3| l.x < 0.0 && l.x > -CARVE_DEPTH && l.y.abs() <= HALF_WIDTH && l.z.abs() <= HALF_HEIGHT;
        let local = corners.map(|c| from.to_local(c));
        let through = if local.iter().all(|&l| in_tube(l)) {
            true
        } else if local.iter().all(|l| l.x >= 0.0) {
            false
        } else {
            in_tube(from.to_local(eye))
        };
        if through {
            let m = transfer(&from, &to);
            return (m.transform_point3(eye), Quat::from_mat3a(&m.matrix3).normalize() * rot);
        }
    }
    (eye, rot)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(mins: Vec3, maxs: Vec3) -> ConvexVolume {
        ConvexVolume::from_box(mins, maxs, source_formats::bsp::contents::SOLID, -1)
    }

    #[test]
    fn opening_cuts_the_wall_behind_an_offset_portal() {
        // sp_a1_intro1's orange portal hangs 2 units in front of its wall.
        let world = CollisionWorld::from_statics(vec![
            solid(Vec3::new(-1135.0, 4200.0, 2600.0), Vec3::new(-1100.0, 4500.0, 2900.0)),
            // The floor runs under the wall, just below the opening.
            solid(Vec3::new(-1400.0, 4200.0, 2600.0), Vec3::new(-1100.0, 4500.0, 2705.0)),
            // A side wall meets the opening's edge.
            solid(Vec3::new(-1300.0, 4384.0, 2705.0), Vec3::new(-1135.0, 4400.0, 2900.0)),
        ]);
        let frame = PortalFrame { center: Vec3::new(-1137.0, 4352.0, 2762.0), forward: Vec3::NEG_X, up: Vec3::Z };
        assert_eq!(carved_statics(&frame, &world), vec![0]);
    }

    /// The player's box below an eye at `eye` (standing: the eye 28 units
    /// above the box's centre).
    fn body_below(eye: Vec3) -> (Vec3, Vec3) {
        (eye - Vec3::Z * 28.0, Vec3::new(16.0, 16.0, 36.0))
    }

    fn view(frames: &[PortalFrame; 2], eye: Vec3, rot: Quat) -> (Vec3, Quat) {
        let (body_center, body_half) = body_below(eye);
        view_through_portals(frames, eye, rot, 4.0, Vec2::new(4.0, 2.25), body_center, body_half)
    }

    #[test]
    fn eye_behind_the_entry_looks_out_of_the_exit() {
        let a = PortalFrame { center: Vec3::ZERO, forward: Vec3::X, up: Vec3::Z };
        let b = PortalFrame { center: Vec3::new(500.0, 0.0, 0.0), forward: Vec3::Y, up: Vec3::Z };
        // Looking along the wall from inside it, just behind A.
        let (eye, rot) = view(&[a, b], Vec3::new(-2.0, 0.0, 10.0), Quat::from_rotation_z(1.0));
        assert!(b.to_local(eye).x > 1.9 && (b.to_local(eye).z - 10.0).abs() < 1e-3);
        assert!(rot.angle_between(Quat::from_mat3a(&transfer(&a, &b).matrix3) * Quat::from_rotation_z(1.0)) < 1e-3);
        // Walking straight in, a few units out: the whole near plane is past the opening.
        let into_a = Quat::from_rotation_z(std::f32::consts::PI);
        let (eye, _) = view(&[a, b], Vec3::new(3.0, 0.0, 10.0), into_a);
        assert!(b.to_local(eye).x < 0.0);
        // Trailing just behind the exit, looking out of it: the view is fine as it is.
        let out_of_b = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let behind_b = b.to_world(Vec3::new(-2.0, 0.0, 10.0));
        assert_eq!(view(&[a, b], behind_b, out_of_b).0, behind_b);
        // Looking along the wall from in front of it: no change.
        let (eye, _) = view(&[a, b], Vec3::new(3.0, 0.0, 10.0), Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
        assert_eq!(eye, Vec3::new(3.0, 0.0, 10.0));
    }

    /// Standing behind a thin wall, lined up with the portal on its other
    /// face, the eye is inside the opening's tube but the body is nowhere
    /// near the opening: the view stays where it is. Jumping up into a
    /// ceiling portal, the eye is as far past the opening with the body
    /// still in it, and goes through.
    #[test]
    fn eye_behind_a_thin_wall_stays_put() {
        let a = PortalFrame { center: Vec3::ZERO, forward: Vec3::X, up: Vec3::Z };
        let b = PortalFrame { center: Vec3::new(500.0, 0.0, 0.0), forward: Vec3::Y, up: Vec3::Z };
        // The wall runs from x = 0 back to x = -8; the player stands against its back.
        let eye = Vec3::new(-30.0, 0.0, 10.0);
        let toward_wall = Quat::IDENTITY;
        assert_eq!(view(&[a, b], eye, toward_wall).0, eye);
        let ceiling = PortalFrame { center: Vec3::new(0.0, 0.0, 100.0), forward: Vec3::NEG_Z, up: Vec3::X };
        let eye = Vec3::new(0.0, 0.0, 110.0);
        let up = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
        assert_ne!(view(&[ceiling, b], eye, up).0, eye);
    }

    #[test]
    fn transfer_maps_entry_to_exit() {
        let a = PortalFrame { center: Vec3::ZERO, forward: Vec3::X, up: Vec3::Z };
        let b = PortalFrame { center: Vec3::new(100.0, 0.0, 0.0), forward: Vec3::Y, up: Vec3::Z };
        let m = transfer(&a, &b);
        // A point just behind A (entering) appears just in front of B.
        let p = m.transform_point3(Vec3::new(-1.0, 0.0, 0.0));
        assert!(b.to_local(p).x > 0.9);
        // Moving into A (-X) means moving out of B (+Y).
        let v = m.transform_vector3(Vec3::new(-10.0, 0.0, 0.0));
        assert!(v.abs_diff_eq(Vec3::new(0.0, 10.0, 0.0), 1e-4));
    }

    #[test]
    fn floor_to_wall_turns_fall_into_forward_motion() {
        let floor = PortalFrame { center: Vec3::ZERO, forward: Vec3::Z, up: Vec3::X };
        let wall = PortalFrame { center: Vec3::new(0.0, 500.0, 64.0), forward: -Vec3::Y, up: Vec3::Z };
        let v = transfer(&floor, &wall).transform_vector3(Vec3::new(0.0, 0.0, -400.0));
        assert!(v.abs_diff_eq(Vec3::new(0.0, -400.0, 0.0), 1e-3), "{v:?}");
    }
}
