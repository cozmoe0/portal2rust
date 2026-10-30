//! Where a portal shot lands: surface validity, fitting the portal opening
//! onto a flat portalable area, nudging it away from edges and obstacles,
//! and snapping to placement helpers.

use bevy::prelude::*;
use source_formats::bsp::contents;

use portal2_world::collision::Physics;
use portal2_world::collision::portal::{HALF_HEIGHT, HALF_WIDTH, PortalFrame};

pub const MAX_SHOT_DISTANCE: f32 = 32768.0;
/// Shots pass through grates and player clips but stop at glass.
pub const SHOT_MASK: u32 = contents::SOLID | contents::MOVEABLE | contents::WINDOW | contents::MONSTER;

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
    /// Whether a surface accepts portals: texinfo index from a trace,
    /// the hit point and normal (portal gel makes any surface take them).
    pub portalable: &'a dyn Fn(i32, Vec3, Vec3) -> bool,
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
        (self.portalable)(tr.surface, tr.end, tr.normal)
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

    /// A wall portal whose bottom edge ends within 1.5 units above a floor
    /// drops down to rest exactly on it.
    fn settle_on_floor(&self, frame: &mut PortalFrame) {
        if frame.up.z <= 0.7 {
            return;
        }
        let start = frame.center + frame.forward * 0.05;
        let mask = contents::SOLID | contents::MOVEABLE | contents::WINDOW | contents::GRATE;
        let far = self.physics.trace(start, start - frame.up * (HALF_HEIGHT + 1.5), Vec3::ZERO, Vec3::ZERO, mask, self.ignore);
        if far.fraction >= 1.0 {
            return;
        }
        let near = self.physics.trace(start, start - frame.up * (HALF_HEIGHT - 0.1), Vec3::ZERO, Vec3::ZERO, mask, self.ignore);
        if near.fraction >= 1.0 {
            frame.center = far.end + frame.up * HALF_HEIGHT - frame.forward * 0.05;
        }
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
        if !(self.portalable)(tr.surface, tr.end, tr.normal) {
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
                let q = portal2_core::coords::angles_to_quat(a);
                let helper_up = q * Vec3::Z;
                let u = helper_up - n * helper_up.dot(n);
                if u.length_squared() > 1e-3 {
                    frame.up = u.normalize();
                }
            }
        }

        let shot_at = frame.center;
        for _ in 0..24 {
            let fails = self.failures(&frame);
            let overlap = self.overlap_push(&frame);
            if fails.is_empty() && overlap.is_none() {
                // The game gives up when fitting moved the portal further
                // than about the opening's half diagonal from where the
                // shot landed.
                let limit2 = ((HALF_HEIGHT * 2.0).powi(2) + (HALF_WIDTH * 2.0).powi(2)) * 0.5;
                if frame.center.distance_squared(shot_at) > limit2 {
                    return Err(PlacementFail::NoFit);
                }
                self.settle_on_floor(&mut frame);
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
