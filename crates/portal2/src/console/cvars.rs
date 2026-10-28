//! Console variables the engine reads, with the original defaults and flags
//! (taken from the installed game DLLs, see `docs/re/cvars.md`).
//!
//! Variables not listed here can still be set (config files set hundreds);
//! they are stored as plain strings.

/// `FCVAR_*` flag bits that matter here.
pub mod flags {
    pub const ARCHIVE: u32 = 1 << 7;
    pub const REPLICATED: u32 = 1 << 13;
    pub const CHEAT: u32 = 1 << 14;
}

#[derive(Clone, Debug)]
pub struct Cvar {
    pub name: String,
    pub value: String,
    pub default: String,
    pub flags: u32,
    /// Registered by the engine (false for values only seen in config files).
    pub known: bool,
}

impl Cvar {
    pub fn f32(&self) -> f32 {
        source_formats::kv::parse_f32_lenient(&self.value)
    }

    pub fn i32(&self) -> i32 {
        source_formats::kv::parse_i32_lenient(&self.value)
    }

    pub fn bool(&self) -> bool {
        self.f32() != 0.0
    }
}

/// Name, default value, flags.
pub const DEFAULTS: &[(&str, &str, u32)] = &[
    // Engine and server.
    ("developer", "0", 0x0),
    ("sv_cheats", "0", 0x82100),
    ("host_timescale", "1.0", 0x6000),
    ("host_framerate", "0", 0x0),
    ("fps_max", "300", 0x0),
    ("sv_unlockedchapters", "1", 0x80),
    // Movement.
    ("sv_gravity", "600", 0x6100),
    ("sv_friction", "4", 0x2102),
    ("sv_stopspeed", "100", 0x2102),
    ("sv_accelerate", "10", 0x2102),
    ("sv_airaccelerate", "10", 0x2102),
    ("sv_maxspeed", "320", 0x2102),
    ("sv_stepsize", "18", 0x2102),
    ("sv_maxvelocity", "3500", 0x2002),
    ("sv_player_funnel_into_portals", "1", 0x2000),
    ("sv_player_funnel_gimme_dot", "0.9", 0x2000),
    ("sv_player_funnel_height_adjust", "128.f", 0x2000),
    ("sv_player_funnel_snap_threshold", "10.f", 0x2000),
    ("sv_player_funnel_speed_bonus", "2.f", 0x6000),
    ("sv_paintairacceleration", "5.0f", 0x6000),
    // Paint (docs/re/paint.md).
    ("bounce_paint_color", "0 165 255 255", 0x2000),
    ("speed_paint_color", "255 106 0 255", 0x2000),
    ("portal_paint_color", "128 128 128 255", 0x2000),
    ("erase_color", "0 0 0 0", 0x2000),
    ("paintbomb_explosion_radius", "100.f", 0x4000),
    ("paintbomb_horizontal_angle_split", "8", 0x4000),
    ("paintbomb_vertical_angle_split", "8", 0x4000),
    ("paintbomb_streak_speed_min", "150.f", 0x4000),
    ("paintbomb_streak_speed_max", "250.f", 0x4000),
    ("sv_paint_alpha_coat", "0.4f", 0x4002),
    ("sv_paint_surface_sphere_radius", "60.f", 0x2),
    ("sv_erase_surface_sphere_radius", "80.f", 0x2),
    ("sv_paint_detection_sphere_radius", "16.f", 0x6000),
    ("paintsplat_bias", "0.1f", 0x6000),
    ("paintsplat_noise_enabled", "1", 0x6000),
    ("paintsplat_max_alpha_noise", "0.1f", 0x6000),
    ("paint_max_surface_border_alpha", "0.7f", 0x2002),
    ("paint_alpha_offset_enabled", "1", 0x2002),
    ("paint_min_valid_alpha_value", "0.7f", 0x2002),
    ("paintblob_gravity_scale", "1.0f", 0x6000),
    ("paintblob_air_drag", "0.1f", 0x6000),
    ("paintblob_streak_angle_threshold", "45.0f", 0x6000),
    ("paintblob_minimum_portal_exit_velocity", "225.0f", 0x6000),
    ("bounce_paint_min_speed", "500.0f", 0x6000),
    ("bounce_paint_wall_jump_upward_speed", "275", 0x6000),
    ("sv_wall_bounce_trade", "0.73", 0x6000),
    ("sv_bounce_paint_forward_velocity_bonus", "0.375f", 0x2002),
    ("sv_portal_placement_on_paint", "1", 0x6000),
    ("sv_press_jump_to_bounce", "3", 0x2000),
    ("sv_bounce_reflect_enabled", "0", 0x6000),
    ("bounce_reflect_restitution", "1.0", 0x6002),
    ("bounce_reflect_wall_jumps_enabled", "1", 0x6002),
    ("bounce_reflect_wall_jump_min_up_speed", "10.0", 0x6002),
    ("bounce_auto_trigger_min_speed", "500", 0x2002),
    ("trampoline_bounce_min_impact_speed", "4.0", 0x6002),
    ("jump_button_can_activate_trampoline_bounce", "1", 0x2002),
    ("trampoline_bounce_off_walls_while_on_ground", "0", 0x2002),
    ("look_dependent_auto_long_jump_enabled", "1", 0x2002),
    ("look_dependent_auto_long_jump_min_cos_angle", "0.7", 0x2002),
    ("sv_wall_jump_help", "1", 0x2000),
    ("sv_wall_jump_help_threshold", "9.0f", 0x2000),
    ("sv_wall_jump_help_amount", "5.0f", 0x2000),
    ("sv_speed_normal", "175.f", 0x6000),
    ("sv_speed_paint_max", "800.0f", 0x6000),
    ("sv_speed_paint_acceleration", "500.0f", 0x6000),
    ("sv_speed_paint_ramp_acceleration", "1000.0f", 0x6000),
    ("sv_speed_paint_side_move_factor", "0.5f", 0x6000),
    ("sv_speed_paint_straf_accel_scale", "0.7f", 0x2002),
    ("sv_speed_paint_on_bounce_deceleration_delay", "0.2f", 0x6000),
    ("sv_props_funnel_into_portals", "1", 0x4000),
    ("sv_props_funnel_into_portals_deceleration", "2.0f", 0x4000),
    ("cl_forwardspeed", "175", 0x4000),
    ("cl_backspeed", "175", 0x4000),
    ("cl_sidespeed", "175", 0x4000),
    ("cl_upspeed", "320", 0x4000),
    // Mouse and view.
    ("sensitivity", "3", 0x80),
    ("m_yaw", "0.022", 0x80),
    ("m_pitch", "0.022", 0x8080),
    ("m_rawinput", "1", 0x80),
    ("cl_pitchup", "89", 0x4000),
    ("cl_pitchdown", "89", 0x4000),
    ("fov_desired", "75", 0x280),
    ("cl_viewmodelfov", "50", 0x2),
    ("cl_bob", "0.002", 0x0),
    ("cl_bobcycle", "0.8", 0x0),
    ("cl_bobup", "0.5", 0x0),
    // HUD and captions.
    ("cl_drawhud", "1", 0x4000),
    ("hidehud", "0", 0x4000),
    ("crosshair", "1", 0x80),
    ("r_drawviewmodel", "1", 0x4000),
    ("cl_showfps", "0", 0x80000),
    ("cl_showpos", "0", 0x80000),
    ("closecaption", "0", 0x1000280),
    ("cc_subtitles", "0", 0x1000080),
    ("gameinstructor_enable", "1", 0x88),
    // Rendering.
    ("mat_hdr_level", "2", 0x0),
    ("mat_force_tonemap_scale", "0.0", 0x4000),
    ("mat_dynamic_tonemapping", "1", 0x4000),
    ("mat_autoexposure_min", "0.5", 0x0),
    ("mat_autoexposure_max", "2", 0x0),
    ("mat_hdr_manual_tonemap_rate", "1.0", 0x0),
    ("mat_accelerate_adjust_exposure_down", "3.0", 0x4000),
    ("mat_tonemap_percent_target", "60.0", 0x4000),
    ("mat_tonemap_percent_bright_pixels", "2.0", 0x4000),
    ("mat_tonemap_min_avglum", "3.0", 0x4000),
    ("mat_bloomscale", "1", 0x0),
    ("mat_motion_blur_enabled", "1", 0x0),
    ("r_portal_stencil_depth", "2", 0x88),
    // Portals and carrying.
    ("portalgun_fire_delay", "0.20", 0x6000),
    ("portal_draw_ghosting", "1", 0x0),
    ("player_held_object_distance", "15", 0x2002),
    ("player_held_object_min_distance", "50", 0x2002),
    ("player_held_object_offset_up_cube", "-10", 0x2002),
    ("player_held_object_offset_up_sphere", "-15", 0x2002),
    ("player_held_object_look_down_adjustment", "20", 0x2002),
    ("player_held_object_max_throw_magnitude", "60", 0x6000),
    ("player_held_object_max_knock_magnitude", "30", 0x6000),
    ("player_held_object_collide_with_player", "0", 0x6000),
    ("player_hold_object_in_column", "1", 0x2002),
    ("player_hold_column_max_size", "96", 0x2002),
];
