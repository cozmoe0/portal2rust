//! What the gels do to the player: bounce gel throws them off the surface,
//! speed gel raises their top speed. Pure functions over the game's rules
//! (docs/re/paint.md); player/movement.rs finds the contacts and calls them.

use bevy::prelude::*;

use crate::console::Console;

/// The gel console variables, read once per tick.
#[derive(Clone, Copy, Debug)]
pub struct GelSettings {
    pub detection_radius: f32,
    pub min_valid_amount: f32,
    pub min_speed: f32,
    pub wall_up_speed: f32,
    pub wall_trade: f32,
    pub forward_bonus: f32,
    pub restitution: f32,
    pub press_jump_mode: i32,
    pub reflect_enabled: bool,
    pub reflect_wall_jumps: bool,
    pub reflect_wall_min_up: f32,
    pub auto_trigger_speed: f32,
    pub min_impact_speed: f32,
    pub jump_activates: bool,
    pub walls_on_ground: bool,
    pub look_dependent: bool,
    pub look_min_cos: f32,
    pub wall_help: bool,
    pub wall_help_threshold: f32,
    pub wall_help_amount: f32,
    pub normal_speed: f32,
    pub speed_max: f32,
    pub speed_accel: f32,
    pub speed_ramp_accel: f32,
    pub side_move_factor: f32,
    pub strafe_accel_scale: f32,
    pub bounce_decel_delay: f32,
}

impl GelSettings {
    pub fn from_console(c: &Console) -> Self {
        Self {
            detection_radius: c.f32("sv_paint_detection_sphere_radius"),
            min_valid_amount: c.f32("paint_min_valid_alpha_value"),
            min_speed: c.f32("bounce_paint_min_speed"),
            wall_up_speed: c.f32("bounce_paint_wall_jump_upward_speed"),
            wall_trade: c.f32("sv_wall_bounce_trade"),
            forward_bonus: c.f32("sv_bounce_paint_forward_velocity_bonus"),
            restitution: c.f32("bounce_reflect_restitution"),
            press_jump_mode: c.i32("sv_press_jump_to_bounce"),
            reflect_enabled: c.bool("sv_bounce_reflect_enabled"),
            reflect_wall_jumps: c.bool("bounce_reflect_wall_jumps_enabled"),
            reflect_wall_min_up: c.f32("bounce_reflect_wall_jump_min_up_speed"),
            auto_trigger_speed: c.f32("bounce_auto_trigger_min_speed"),
            min_impact_speed: c.f32("trampoline_bounce_min_impact_speed"),
            jump_activates: c.bool("jump_button_can_activate_trampoline_bounce"),
            walls_on_ground: c.bool("trampoline_bounce_off_walls_while_on_ground"),
            look_dependent: c.bool("look_dependent_auto_long_jump_enabled"),
            look_min_cos: c.f32("look_dependent_auto_long_jump_min_cos_angle"),
            wall_help: c.bool("sv_wall_jump_help"),
            wall_help_threshold: c.f32("sv_wall_jump_help_threshold"),
            wall_help_amount: c.f32("sv_wall_jump_help_amount"),
            normal_speed: c.f32("sv_speed_normal"),
            speed_max: c.f32("sv_speed_paint_max"),
            speed_accel: c.f32("sv_speed_paint_acceleration"),
            speed_ramp_accel: c.f32("sv_speed_paint_ramp_acceleration"),
            side_move_factor: c.f32("sv_speed_paint_side_move_factor"),
            strafe_accel_scale: c.f32("sv_speed_paint_straf_accel_scale"),
            bounce_decel_delay: c.f32("sv_speed_paint_on_bounce_deceleration_delay"),
        }
    }
}

impl Default for GelSettings {
    fn default() -> Self {
        Self {
            detection_radius: 16.0,
            min_valid_amount: 0.7,
            min_speed: 500.0,
            wall_up_speed: 275.0,
            wall_trade: 0.73,
            forward_bonus: 0.375,
            restitution: 1.0,
            press_jump_mode: 3,
            reflect_enabled: false,
            reflect_wall_jumps: true,
            reflect_wall_min_up: 10.0,
            auto_trigger_speed: 500.0,
            min_impact_speed: 4.0,
            jump_activates: true,
            walls_on_ground: false,
            look_dependent: true,
            look_min_cos: 0.7,
            wall_help: true,
            wall_help_threshold: 9.0,
            wall_help_amount: 5.0,
            normal_speed: 175.0,
            speed_max: 800.0,
            speed_accel: 500.0,
            speed_ramp_accel: 1000.0,
            side_move_factor: 0.5,
            strafe_accel_scale: 0.7,
            bounce_decel_delay: 0.2,
        }
    }
}

/// The player's situation when touching bounce gel.
#[derive(Clone, Copy, Debug)]
pub struct BounceCheck {
    /// Velocity before this tick's move.
    pub velocity: Vec3,
    /// Contact normal (out of the surface).
    pub normal: Vec3,
    pub up: Vec3,
    /// In the air before this tick's move.
    pub in_air: bool,
    pub jump_held: bool,
    pub ducked: bool,
    /// The player's current top speed (raised by speed gel).
    pub max_speed: f32,
    pub eye_forward: Vec3,
}

/// Whether touching bounce gel bounces (`sv_press_jump_to_bounce` 3): when
/// coming into it from the air, when jump is held, or when running onto it
/// fast while looking along the run (auto long jump). Walls touched from
/// the ground need jump. Crouching suppresses it.
pub fn should_bounce(c: &BounceCheck, g: &GelSettings) -> bool {
    if c.ducked {
        return false;
    }
    let into = c.velocity.dot(c.normal);
    let wall = c.up.dot(c.normal).abs() < 1e-4;
    let walls_ok = g.walls_on_ground || c.in_air;
    let jump = c.jump_held && g.jump_activates;
    let long_jump = !c.in_air
        && c.max_speed > g.auto_trigger_speed
        && (!g.look_dependent || c.velocity.normalize_or_zero().dot(c.eye_forward) >= g.look_min_cos);
    match g.press_jump_mode {
        0 => true,
        1 | 2 => c.jump_held,
        _ => {
            if (-g.min_impact_speed <= into || (!c.in_air && c.normal.z > 0.0)) && !jump && !long_jump {
                return false;
            }
            if wall && !walls_ok {
                return jump;
            }
            true
        }
    }
}

/// The push off a surface: along the normal at `speed`, and on walls a
/// lift along `up` traded against part of the outward push.
fn impulse(normal: Vec3, up: Vec3, speed: f32, g: &GelSettings) -> Vec3 {
    let mut out = Vec3::ZERO;
    let mut k = 0.0;
    if g.press_jump_mode != 3 || g.reflect_wall_jumps {
        let d = up.dot(normal);
        if d <= -0.1 {
            let side = (normal - up * d).normalize_or_zero();
            k = normal.dot(side);
            out += side * g.wall_up_speed * k;
        } else {
            k = 1.0 - d;
            out += up * g.wall_up_speed * k;
        }
    }
    out + normal * speed * ((1.0 - k) * (1.0 - g.wall_trade) + g.wall_trade)
}

/// Turns a horizontal bounce off a wall that is nearly straight out of it
/// onto the wall's normal (fully within the help amount, by the amount up
/// to the threshold).
fn wall_jump_help(v: Vec3, normal: Vec3, g: &GelSettings) -> Vec3 {
    if !g.wall_help {
        return v;
    }
    let h = v.truncate().normalize_or_zero();
    let n = normal.truncate().normalize_or_zero();
    if h == Vec2::ZERO || n == Vec2::ZERO {
        return v;
    }
    let angle = h.dot(n).clamp(-1.0, 1.0).acos().to_degrees();
    let turn = if angle <= g.wall_help_amount {
        angle
    } else if angle <= g.wall_help_threshold {
        g.wall_help_amount
    } else {
        return v;
    };
    let sign = if n.y * h.x - n.x * h.y <= 0.0 { -1.0 } else { 1.0 };
    let r = Vec2::from_angle((turn * sign).to_radians()).rotate(v.truncate());
    Vec3::new(r.x, r.y, v.z)
}

/// Velocity after bouncing off a surface with `normal`.
pub fn bounce_velocity(v: Vec3, normal: Vec3, up: Vec3, g: &GelSettings) -> Vec3 {
    let into = v.dot(normal);
    let tangent = v - normal * into;
    let mut out = if (g.reflect_enabled || g.press_jump_mode == 3) && into.abs() * g.restitution >= g.min_speed {
        // Fast enough to reflect; vertical walls still give the wall jump.
        let mut push = normal * (-into * g.restitution);
        let wall = normal.z.abs() <= 0.001;
        if g.reflect_wall_jumps && wall {
            push = impulse(normal, up, into.abs(), g);
            if g.reflect_wall_min_up > tangent.z + push.z {
                push.z += g.reflect_wall_min_up - tangent.z;
            }
            wall_jump_help(tangent + push, normal, g)
        } else {
            tangent + push
        }
    } else {
        let flat = tangent - up * up.dot(tangent);
        let push = impulse(normal, up, g.min_speed, g);
        let along = flat.normalize_or_zero() * flat.dot(push.normalize_or_zero());
        wall_jump_help(flat - along + push, normal, g)
    };
    // Floors add part of the horizontal speed, up to 1 + bonus times the
    // normal speed.
    if (normal.z - 1.0).abs() < 1e-5 {
        let h = Vec3::new(out.x, out.y, 0.0);
        let cap = (g.forward_bonus + 1.0) * g.normal_speed;
        if cap * cap > h.length_squared() {
            let mut add = h * g.forward_bonus;
            if (h + add).length_squared() > cap * cap {
                add = h.normalize_or_zero() * cap - h;
            }
            out += add;
        }
    }
    out
}

/// The player's situation on speed gel.
#[derive(Clone, Copy, Debug)]
pub struct SpeedCheck {
    pub max_speed: f32,
    pub velocity: Vec3,
    /// Wish direction (zero without input).
    pub wish: Vec3,
    pub on_ground: bool,
    pub ground_normal: Vec3,
    /// Horizontal view forward.
    pub forward: Vec3,
}

/// Top speed after a tick on speed gel.
pub fn speed_on_gel(c: &SpeedCheck, g: &GelSettings, dt: f32) -> f32 {
    let w = c.wish.normalize_or_zero();
    let moving = c.velocity.abs().max_element() >= 0.01;
    if w == Vec3::ZERO || !moving {
        return g.normal_speed;
    }
    let along_ground = (w - c.ground_normal * w.dot(c.ground_normal)).normalize_or_zero();
    let uphill = along_ground.z > 0.0 && along_ground.z < 0.999_999;
    let accel = if !c.on_ground {
        0.0
    } else if uphill {
        g.speed_ramp_accel
    } else {
        g.speed_accel
    };
    let scale = if c.forward.dot(w).abs() > 0.9 { g.strafe_accel_scale } else { 1.0 };
    let speed = c.velocity.length();
    let align = c.velocity.normalize_or_zero().dot(w);
    let change = if align >= 0.0 { align * accel * scale * dt } else { (speed - g.normal_speed).abs() * align };
    let raised = (c.max_speed + change).clamp(g.normal_speed, g.speed_max);
    if align > 0.0 && align < 0.3 {
        // Turning hard: toward the speed along the new direction.
        let ahead = c.velocity.dot(w);
        return ((speed - ahead) * 0.3 + ahead).max(g.normal_speed);
    }
    raised
}

/// Top speed after a tick off speed gel: on the ground (or touching
/// nothing) it falls at the acceleration rate, after a short delay on
/// bounce gel; in the air against a surface it follows the speed along it.
pub fn speed_off_gel(c: &SpeedCheck, touching: Option<Vec3>, on_bounce: bool, delay: &mut f32, g: &GelSettings, dt: f32) -> f32 {
    let new = match touching {
        Some(n) if !c.on_ground => (c.velocity - n * c.velocity.dot(n)).length(),
        _ => {
            if on_bounce && *delay < g.bounce_decel_delay {
                *delay += dt;
                c.max_speed
            } else {
                let w = c.wish.normalize_or_zero();
                if w == Vec3::ZERO {
                    g.normal_speed
                } else {
                    let align = c.velocity.normalize_or_zero().dot(w);
                    if align > 0.0 && align < 0.3 {
                        let ahead = c.velocity.dot(w);
                        ((c.velocity.length() - ahead) * 0.3 + ahead).max(g.normal_speed)
                    } else {
                        c.max_speed - g.speed_accel * dt
                    }
                }
            }
        }
    };
    if !on_bounce {
        *delay = 0.0;
    }
    new.clamp(g.normal_speed, g.speed_max)
}

/// Forward and side input scales while the top speed is raised: on the
/// ground with both pressed, the one less aligned with the velocity is
/// weakened by the side-move factor.
pub fn move_axis_scales(
    forward_dir: Vec3,
    right_dir: Vec3,
    velocity: Vec3,
    forward: f32,
    right: f32,
    on_ground: bool,
    g: &GelSettings,
) -> (f32, f32) {
    if !on_ground || forward == 0.0 || right == 0.0 {
        return (1.0, 1.0);
    }
    if forward_dir.dot(velocity).abs() <= right_dir.dot(velocity).abs() { (g.side_move_factor, 1.0) } else { (1.0, g.side_move_factor) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g() -> GelSettings {
        GelSettings::default()
    }

    fn falling(onto: Vec3, v: Vec3) -> BounceCheck {
        BounceCheck {
            velocity: v,
            normal: onto,
            up: Vec3::Z,
            in_air: true,
            jump_held: false,
            ducked: false,
            max_speed: 175.0,
            eye_forward: Vec3::X,
        }
    }

    #[test]
    fn landing_on_bounce_gel_bounces_walking_onto_it_does_not() {
        assert!(should_bounce(&falling(Vec3::Z, Vec3::new(0.0, 0.0, -300.0)), &g()));
        let mut walk = falling(Vec3::Z, Vec3::new(175.0, 0.0, 0.0));
        walk.in_air = false;
        assert!(!should_bounce(&walk, &g()));
        walk.jump_held = true;
        assert!(should_bounce(&walk, &g()));
        walk.ducked = true;
        assert!(!should_bounce(&walk, &g()));
        // Running onto it on speed gel, looking ahead: long jump.
        let mut run = falling(Vec3::Z, Vec3::new(700.0, 0.0, 0.0));
        run.in_air = false;
        run.max_speed = 700.0;
        assert!(should_bounce(&run, &g()));
        run.eye_forward = Vec3::Y;
        assert!(!should_bounce(&run, &g()));
    }

    #[test]
    fn floor_bounce_launches_at_min_speed_with_forward_bonus() {
        let v = bounce_velocity(Vec3::new(100.0, 0.0, -200.0), Vec3::Z, Vec3::Z, &g());
        assert!((v.z - 500.0).abs() < 1e-3);
        assert!((v.x - 137.5).abs() < 1e-3);
        // The bonus stops at 1.375 × 175.
        let v = bounce_velocity(Vec3::new(200.0, 0.0, -200.0), Vec3::Z, Vec3::Z, &g());
        assert!((v.x - 240.625).abs() < 1e-3);
        // Fast falls reflect.
        let v = bounce_velocity(Vec3::new(0.0, 0.0, -900.0), Vec3::Z, Vec3::Z, &g());
        assert!((v.z - 900.0).abs() < 1e-3);
    }

    #[test]
    fn wall_bounce_pushes_out_and_up() {
        let v = bounce_velocity(Vec3::new(-200.0, 0.0, -50.0), Vec3::X, Vec3::Z, &g());
        assert!((v.x - 500.0 * 0.73).abs() < 1e-2, "{v}");
        assert!((v.z - 275.0).abs() < 1e-2, "{v}");
    }

    #[test]
    fn speed_gel_raises_and_resets_top_speed() {
        let mut c = SpeedCheck {
            max_speed: 175.0,
            velocity: Vec3::new(175.0, 0.0, 0.0),
            wish: Vec3::X,
            on_ground: true,
            ground_normal: Vec3::Z,
            forward: Vec3::Y,
        };
        let up = speed_on_gel(&c, &g(), 0.1);
        assert!((up - 225.0).abs() < 1e-3);
        c.max_speed = 790.0;
        assert_eq!(speed_on_gel(&c, &g(), 0.1), 800.0);
        c.wish = Vec3::ZERO;
        assert_eq!(speed_on_gel(&c, &g(), 0.1), 175.0);
        // Off the gel it falls back at the acceleration rate.
        c.wish = Vec3::X;
        c.max_speed = 600.0;
        let mut delay = 0.0;
        assert!((speed_off_gel(&c, None, false, &mut delay, &g(), 0.1) - 550.0).abs() < 1e-3);
    }
}
