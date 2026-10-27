//! Where a portal shot lands: surface validity, fitting the portal opening
//! onto a flat portalable area, nudging it away from edges and obstacles,
//! and snapping to placement helpers.

use std::f32::consts::PI;

use bevy::math::Affine3A;
use bevy::prelude::*;
use source_formats::bsp::contents;

use crate::collision::Physics;

pub const HALF_WIDTH: f32 = 32.0;
pub const HALF_HEIGHT: f32 = 56.0;
pub const MAX_SHOT_DISTANCE: f32 = 32768.0;
/// Shots pass through grates and player clips but stop at glass.
pub const SHOT_MASK: u32 = contents::SOLID | contents::MOVEABLE | contents::WINDOW | contents::MONSTER;

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
}

/// Maps space in front of `from` to space in front of `to` (entering one
/// portal means leaving the other, turned around).
pub fn transfer(from: &PortalFrame, to: &PortalFrame) -> Affine3A {
    to.affine() * Affine3A::from_rotation_z(PI) * from.affine().inverse()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementFail {
    NoSurface,
    /// Hit a moving or model entity.
    OnEntity,
    /// Hit geometry without a portal-capable material (props, terrain).
    NoMaterial,
    /// The surface material does not accept portals.
    NotPortalable,
    Fizzler,
    NoFit,
}

/// Placement helper entity data (snaps nearby shots).
#[derive(Clone, Copy, Debug)]
pub struct Helper {
    pub origin: Vec3,
    pub angles: Option<Vec3>,
    pub radius: f32,
}

pub struct PlacementContext<'a, 'w, 's> {
    pub physics: &'a Physics<'w, 's>,
    pub ignore: &'a [Entity],
    /// Whether a surface (texinfo index from a trace) accepts portals.
    pub portalable: &'a dyn Fn(i32) -> bool,
    pub noportal_volumes: &'a [Entity],
    pub bumpers: &'a [Entity],
    pub cleansers: &'a [Entity],
    pub helpers: &'a [Helper],
    /// The other portal of the pair, which this one must not overlap.
    pub other: Option<PortalFrame>,
}

impl PlacementContext<'_, '_, '_> {
    fn surface_ok(&self, p: Vec3, frame: &PortalFrame) -> bool {
        let n = frame.forward;
        let tr = self.physics.trace(p + n * 4.0, p - n * 4.0, Vec3::ZERO, Vec3::ZERO, SHOT_MASK, self.ignore);
        if tr.start_solid || tr.fraction >= 1.0 || tr.entity.is_some() {
            return false;
        }
        if tr.normal.dot(n) < 0.99 || (n.dot(tr.end) - frame.plane_dist()).abs() > 0.5 {
            return false;
        }
        (self.portalable)(tr.surface)
    }

    fn in_any(&self, list: &[Entity], p: Vec3) -> bool {
        let r = Vec3::splat(0.5);
        list.iter().any(|&e| self.physics.touches(e, p, -r, r))
    }

    /// Opening sample offsets (left, up) that fail, for the bump direction.
    fn failures(&self, frame: &PortalFrame) -> Vec<Vec2> {
        let mut out = Vec::new();
        for iy in -2..=2 {
            for iz in -3..=3 {
                let s = Vec2::new(iy as f32 / 2.0 * (HALF_WIDTH - 1.0), iz as f32 / 3.0 * (HALF_HEIGHT - 1.0));
                // Corners of the rectangle fall outside the oval opening.
                if (s.x / HALF_WIDTH).powi(2) + (s.y / HALF_HEIGHT).powi(2) > 1.05 {
                    continue;
                }
                let p = frame.to_world(Vec3::new(0.0, s.x, s.y));
                let front = p + frame.forward * 1.0;
                if !self.surface_ok(p, frame) || self.in_any(self.noportal_volumes, front) || self.in_any(self.bumpers, front) {
                    out.push(s);
                }
            }
        }
        out
    }

    /// Offset (left, up) pushing this frame off the other portal, if they overlap.
    fn overlap_push(&self, frame: &PortalFrame) -> Option<Vec2> {
        let other = self.other?;
        if other.forward.dot(frame.forward) < 0.99 || (other.plane_dist() - frame.plane_dist()).abs() > 1.0 {
            return None;
        }
        let l = frame.to_local(other.center);
        let overlap = l.y.abs() < HALF_WIDTH * 2.0 && l.z.abs() < HALF_HEIGHT * 2.0;
        if !overlap {
            return None;
        }
        // Move away along whichever axis needs the smaller correction.
        let dy = HALF_WIDTH * 2.0 - l.y.abs() + 1.0;
        let dz = HALF_HEIGHT * 2.0 - l.z.abs() + 1.0;
        Some(if dy < dz { Vec2::new(-l.y.signum() * dy, 0.0) } else { Vec2::new(0.0, -l.z.signum() * dz) })
    }

    fn crosses_cleanser(&self, start: Vec3, end: Vec3) -> bool {
        let len = start.distance(end);
        let steps = (len / 8.0).ceil().max(1.0) as usize;
        (0..=steps).any(|i| {
            let p = start.lerp(end, i as f32 / steps as f32);
            self.in_any(self.cleansers, p)
        })
    }

    /// Places a portal shot from `start` along `dir`; `view_forward` orients
    /// floor and ceiling portals.
    pub fn place(&self, start: Vec3, dir: Vec3, view_forward: Vec3) -> Result<PortalFrame, PlacementFail> {
        let end = start + dir * MAX_SHOT_DISTANCE;
        let tr = self.physics.trace(start, end, Vec3::ZERO, Vec3::ZERO, SHOT_MASK, self.ignore);
        if tr.fraction >= 1.0 || tr.start_solid {
            return Err(PlacementFail::NoSurface);
        }
        if self.crosses_cleanser(start, tr.end) {
            return Err(PlacementFail::Fizzler);
        }
        if tr.entity.is_some() {
            return Err(PlacementFail::OnEntity);
        }
        if tr.surface < 0 {
            return Err(PlacementFail::NoMaterial);
        }
        if !(self.portalable)(tr.surface) {
            return Err(PlacementFail::NotPortalable);
        }
        let n = tr.normal.normalize();
        let up = if n.z.abs() > 0.7 {
            let f = view_forward - n * view_forward.dot(n);
            let f = if f.length_squared() < 1e-4 { Vec3::X - n * n.x } else { f };
            // On floors the top of the opening points away from the shooter;
            // on ceilings toward them.
            let f = f.normalize();
            if n.z > 0.0 { f } else { -f }
        } else {
            (Vec3::Z - n * n.z).normalize()
        };
        let mut frame = PortalFrame { center: tr.end, forward: n, up };

        if let Some(h) = self.helpers.iter().find(|h| {
            let d = h.origin - tr.end;
            d.dot(n).abs() < 8.0 && (d - n * d.dot(n)).length() <= h.radius
        }) {
            frame.center = h.origin - n * (h.origin - tr.end).dot(n);
            if let Some(a) = h.angles {
                let q = crate::coords::angles_to_quat(a);
                let helper_up = q * Vec3::Z;
                let u = helper_up - n * helper_up.dot(n);
                if u.length_squared() > 1e-3 {
                    frame.up = u.normalize();
                }
            }
        }

        for _ in 0..24 {
            let fails = self.failures(&frame);
            let overlap = self.overlap_push(&frame);
            if fails.is_empty() && overlap.is_none() {
                return Ok(frame);
            }
            let mut push = Vec2::ZERO;
            if !fails.is_empty() {
                let avg = fails.iter().fold(Vec2::ZERO, |a, &b| a + b) / fails.len() as f32;
                if avg.length_squared() < 1e-3 {
                    // Failures all around the centre: nothing to bump toward.
                    return Err(PlacementFail::NoFit);
                }
                push -= avg.normalize() * 4.0;
            }
            if let Some(o) = overlap {
                push += o;
            }
            frame.center += frame.left() * push.x + frame.up * push.y;
        }
        Err(PlacementFail::NoFit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
