//! Perspective projection whose near plane can be replaced by an arbitrary
//! view-space plane, so a portal's view starts at the exit portal's surface
//! instead of including the wall behind it.
//!
//! Works with Bevy's reverse-Z infinite projection: depth is 1 on the clip
//! plane, falls toward 0 with distance, and anything on the camera's side of
//! the plane lands above 1 and is clipped.

use bevy::camera::primitives::Frustum;
use bevy::camera::{CameraProjection, SubCameraView};
use bevy::math::Vec3A;
use bevy::prelude::*;

#[derive(Clone, Debug)]
pub struct PortalProjection {
    pub base: PerspectiveProjection,
    /// View-space plane (xyz normal, w offset) whose positive side is kept;
    /// `None` renders like the base projection.
    pub clip_plane: Option<Vec4>,
}

impl PortalProjection {
    pub fn new(base: PerspectiveProjection) -> Self {
        Self { base, clip_plane: None }
    }
}

impl CameraProjection for PortalProjection {
    fn get_clip_from_view(&self) -> Mat4 {
        let mut m = self.base.get_clip_from_view();
        let Some(c) = self.clip_plane else { return m };
        // The camera must sit on the clipped side of the plane.
        if c.w >= 0.0 {
            return m;
        }
        // Scale so that depth stays >= 0 at infinity along every frustum
        // corner ray (directions with z = -1).
        let ty = (self.base.fov * 0.5).tan();
        let tx = ty * self.base.aspect_ratio;
        let mut max_s = 0.0f32;
        for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let d = Vec3::new(sx * tx, sy * ty, -1.0);
            max_s = max_s.max(c.truncate().dot(d));
        }
        let k = if max_s > 1e-6 { 1.0 / max_s } else { 1.0 };
        // z_clip = w_clip - k * (C · v), with w_clip = -z_view.
        m.x_axis.z = -k * c.x;
        m.y_axis.z = -k * c.y;
        m.z_axis.z = -1.0 - k * c.z;
        m.w_axis.z = -k * c.w;
        m
    }

    fn get_clip_from_view_for_sub(&self, sub_view: &SubCameraView) -> Mat4 {
        self.base.get_clip_from_view_for_sub(sub_view)
    }

    fn update(&mut self, width: f32, height: f32) {
        self.base.update(width, height);
    }

    fn far(&self) -> f32 {
        self.base.far
    }

    fn get_frustum_corners(&self, z_near: f32, z_far: f32) -> [Vec3A; 8] {
        self.base.get_frustum_corners(z_near, z_far)
    }

    fn compute_frustum(&self, camera_transform: &GlobalTransform) -> Frustum {
        // Cull with the ordinary frustum; the oblique plane only clips.
        self.base.compute_frustum(camera_transform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn depth(m: Mat4, p: Vec3) -> f32 {
        let c = m * p.extend(1.0);
        c.z / c.w
    }

    #[test]
    fn plane_clips_near_side_and_keeps_far_side() {
        let base = PerspectiveProjection { fov: 1.0, aspect_ratio: 1.0, near: 1.0, ..default() };
        // Plane z = -10 in view space, keeping points further away (z < -10).
        let proj = PortalProjection { base, clip_plane: Some(Vec4::new(0.0, 0.0, -1.0, -10.0)) };
        let m = proj.get_clip_from_view();
        assert!((depth(m, Vec3::new(0.0, 0.0, -10.0)) - 1.0).abs() < 1e-4);
        assert!(depth(m, Vec3::new(0.0, 0.0, -5.0)) > 1.0);
        let far = depth(m, Vec3::new(0.0, 0.0, -100.0));
        assert!(far < 1.0 && far >= 0.0);
        // Depth decreases with distance so depth testing still works.
        assert!(depth(m, Vec3::new(0.0, 0.0, -50.0)) > far);
    }
}
