//! The game runs in Source's native coordinate system (Z up, X forward,
//! units in inches) so map data, movement and physics need no conversion.
//! Only cameras need to know that Bevy views look down their local -Z.

use bevy::prelude::*;

/// Rotation for Source `angles` (pitch, yaw, roll in degrees).
pub fn angles_to_quat(angles: Vec3) -> Quat {
    let (p, y, r) = (angles.x.to_radians(), angles.y.to_radians(), angles.z.to_radians());
    Quat::from_rotation_z(y) * Quat::from_rotation_y(p) * Quat::from_rotation_x(r)
}

/// Forward vector for Source angles.
pub fn angles_forward(angles: Vec3) -> Vec3 {
    angles_to_quat(angles) * Vec3::X
}

/// Inverse of [`angles_to_quat`], returning (pitch, yaw, roll) in degrees.
pub fn quat_to_angles(q: Quat) -> Vec3 {
    let f = q * Vec3::X;
    let l = q * Vec3::Y;
    let u = q * Vec3::Z;
    let xy = (f.x * f.x + f.y * f.y).sqrt();
    if xy > 0.001 {
        let yaw = f.y.atan2(f.x);
        let pitch = (-f.z).atan2(xy);
        let roll = l.z.atan2(u.z);
        Vec3::new(pitch.to_degrees(), yaw.to_degrees(), roll.to_degrees())
    } else {
        let yaw = (-l.x).atan2(l.y);
        let pitch = (-f.z).atan2(xy);
        Vec3::new(pitch.to_degrees(), yaw.to_degrees(), 0.0)
    }
}

/// Maps a Source-space orientation (X forward, Y left, Z up) to the
/// orientation a Bevy camera needs (-Z forward, +Y up, +X right).
pub fn camera_rotation(view: Quat) -> Quat {
    let m = Mat3::from_cols(Vec3::new(0.0, -1.0, 0.0), Vec3::new(0.0, 0.0, 1.0), Vec3::new(-1.0, 0.0, 0.0));
    view * Quat::from_mat3(&m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_matches_source_convention() {
        // Pitch down 90 looks straight down, yaw 90 looks along +Y.
        assert!(angles_forward(Vec3::new(90.0, 0.0, 0.0)).abs_diff_eq(Vec3::NEG_Z, 1e-5));
        assert!(angles_forward(Vec3::new(0.0, 90.0, 0.0)).abs_diff_eq(Vec3::Y, 1e-5));
    }

    #[test]
    fn camera_looks_forward() {
        let q = camera_rotation(angles_to_quat(Vec3::new(0.0, 90.0, 0.0)));
        assert!((q * Vec3::NEG_Z).abs_diff_eq(Vec3::Y, 1e-5));
        assert!((q * Vec3::Y).abs_diff_eq(Vec3::Z, 1e-5));
    }

    #[test]
    fn angles_roundtrip() {
        let a = Vec3::new(30.0, 120.0, 10.0);
        assert!(quat_to_angles(angles_to_quat(a)).abs_diff_eq(a, 1e-3));
    }
}
