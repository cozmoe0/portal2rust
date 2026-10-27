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
];
