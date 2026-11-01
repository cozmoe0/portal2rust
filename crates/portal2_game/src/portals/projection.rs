//! Perspective projection whose near plane can be replaced by an arbitrary
//! view-space plane, so a portal's view starts at the exit portal's surface
//! instead of including the wall behind it.
//!
//! Works with Bevy's reverse-Z infinite projection: depth is 1 on the clip
//! plane, falls toward 0 with distance, and anything on the camera's side of
//! the plane lands above 1 and is clipped.
//!
//! A portal is seen through only where it lies on screen, so a view may
//! also be cropped to a rectangle of the full picture (drawn into the same
//! rectangle of its target as the camera's viewport): every pixel inside
//! comes out as it would in the uncropped view, and nothing outside is
//! drawn. The view culls with that rectangle's frustum, with the exit
//! portal's plane in place of the near plane, the way the game clips what a
//! portal's view draws to the frustum through the portal
//! (`r_portal_use_complex_frustums`, `r_PortalTestEnts`).

use bevy::camera::primitives::Frustum;
use bevy::camera::{CameraProjection, SubCameraView};
use bevy::math::{Rect, Vec3A};
use bevy::prelude::*;
use bevy::shape::{HalfSpace, ViewFrustum};

#[derive(Clone, Debug)]
pub struct PortalProjection {
    pub base: PerspectiveProjection,
    /// View-space plane (xyz normal, w offset) whose positive side is kept;
    /// `None` renders like the base projection.
    pub clip_plane: Option<Vec4>,
    /// The part of the full picture drawn, in normalized device coordinates
    /// (y up); `None` draws all of it.
    pub crop: Option<Rect>,
    /// World-space plane (xyz normal, w offset) everything drawn lies in
    /// front of, culling what is behind it.
    pub cull_plane: Option<Vec4>,
}

impl PortalProjection {
    pub fn new(base: PerspectiveProjection) -> Self {
        Self { base, clip_plane: None, crop: None, cull_plane: None }
    }

    /// Maps the cropped part of clip space onto the whole of it.
    fn apply_crop(&self, m: Mat4) -> Mat4 {
        let Some(r) = self.crop else { return m };
        let size = r.size();
        if size.x <= 0.0 || size.y <= 0.0 {
            return m;
        }
        let scale = 2.0 / size;
        let offset = -(r.min + r.max) / size;
        Mat4::from_cols(
            Vec4::new(scale.x, 0.0, 0.0, 0.0),
            Vec4::new(0.0, scale.y, 0.0, 0.0),
            Vec4::Z,
            Vec4::new(offset.x, offset.y, 0.0, 1.0),
        ) * m
    }
}

impl CameraProjection for PortalProjection {
    fn get_clip_from_view(&self) -> Mat4 {
        let mut m = self.base.get_clip_from_view();
        let Some(c) = self.clip_plane else { return self.apply_crop(m) };
        // The camera must sit on the clipped side of the plane.
        if c.w >= 0.0 {
            return self.apply_crop(m);
        }
        // Scale so that depth stays >= 0 at infinity along every frustum
        // corner ray (directions with z = -1). The uncropped frustum's, so a
        // cropped view's depths are the full view's.
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
        self.apply_crop(m)
    }

    fn get_clip_from_view_for_sub(&self, sub_view: &SubCameraView) -> Mat4 {
        self.base.get_clip_from_view_for_sub(sub_view)
    }

    fn update(&mut self, _width: f32, _height: f32) {
        // The aspect ratio is the main view's (set with `base` every frame):
        // a cropped view's viewport has its own.
    }

    fn far(&self) -> f32 {
        self.base.far
    }

    fn get_frustum_corners(&self, z_near: f32, z_far: f32) -> [Vec3A; 8] {
        self.base.get_frustum_corners(z_near, z_far)
    }

    fn compute_frustum(&self, camera_transform: &GlobalTransform) -> Frustum {
        // Cull with the cropped ordinary frustum; the oblique plane only
        // clips, and the cull plane takes the near plane's place.
        let clip_from_world = self.apply_crop(self.base.get_clip_from_view()) * camera_transform.affine().inverse();
        let mut frustum = ViewFrustum::from_clip_from_world_custom_far(
            &clip_from_world,
            &camera_transform.translation(),
            &camera_transform.back(),
            self.far(),
        );
        if let Some(plane) = self.cull_plane {
            frustum.half_spaces[ViewFrustum::NEAR_PLANE_IDX] = HalfSpace::new(plane);
        }
        Frustum(frustum)
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
        let proj = PortalProjection { clip_plane: Some(Vec4::new(0.0, 0.0, -1.0, -10.0)), ..PortalProjection::new(base) };
        let m = proj.get_clip_from_view();
        assert!((depth(m, Vec3::new(0.0, 0.0, -10.0)) - 1.0).abs() < 1e-4);
        assert!(depth(m, Vec3::new(0.0, 0.0, -5.0)) > 1.0);
        let far = depth(m, Vec3::new(0.0, 0.0, -100.0));
        assert!((0.0..1.0).contains(&far));
        // Depth decreases with distance so depth testing still works.
        assert!(depth(m, Vec3::new(0.0, 0.0, -50.0)) > far);
    }

    /// A cropped view puts each point where the full view does, relative to
    /// the crop, at the same depth.
    #[test]
    fn crop_matches_the_full_view() {
        let base = PerspectiveProjection { fov: 1.2, aspect_ratio: 16.0 / 9.0, near: 1.0, ..default() };
        let full = PortalProjection { clip_plane: Some(Vec4::new(0.1, 0.0, -1.0, -10.0)), ..PortalProjection::new(base) };
        let crop = Rect::new(-0.25, 0.1, 0.5, 0.8);
        let cropped = PortalProjection { crop: Some(crop), ..full.clone() };
        let (a, b) = (full.get_clip_from_view(), cropped.get_clip_from_view());
        for p in [Vec3::new(1.0, 2.0, -20.0), Vec3::new(-3.0, 0.5, -15.0), Vec3::new(0.0, 4.0, -40.0)] {
            let (ca, cb) = (a * p.extend(1.0), b * p.extend(1.0));
            let (na, nb) = (ca.truncate() / ca.w, cb.truncate() / cb.w);
            let expected = (na.truncate() - crop.center()) * 2.0 / crop.size();
            assert!((nb.truncate() - expected).length() < 1e-4);
            assert!((nb.z - na.z).abs() < 1e-6);
        }
    }

    /// The culling frustum is the crop's, with the cull plane as its near
    /// plane.
    #[test]
    fn frustum_follows_crop_and_cull_plane() {
        let base = PerspectiveProjection { fov: 1.2, aspect_ratio: 1.0, near: 1.0, ..default() };
        let proj = PortalProjection {
            crop: Some(Rect::new(0.0, -1.0, 1.0, 1.0)),
            cull_plane: Some(Vec4::new(0.0, 0.0, -1.0, -50.0)),
            ..PortalProjection::new(base)
        };
        let frustum = proj.compute_frustum(&GlobalTransform::IDENTITY);
        let inside = |p: Vec3| frustum.intersects_sphere(&bevy::camera::primitives::Sphere { center: p.into(), radius: 0.1 }, false);
        assert!(inside(Vec3::new(5.0, 0.0, -100.0)), "right half, past the plane");
        assert!(!inside(Vec3::new(-5.0, 0.0, -100.0)), "left half is cropped away");
        assert!(!inside(Vec3::new(5.0, 0.0, -20.0)), "in front of the cull plane");
    }
}
