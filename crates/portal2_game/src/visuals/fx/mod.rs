//! The visual overhaul ("FX"): real-time light, reflections, atmosphere and
//! post-processing layered over the Source-faithful renderer, without
//! touching the game's files or rules (see `docs/visual_overhaul.md`).
//!
//! Everything is driven by one preset, `r_fx_preset` (0 off, 1 medium,
//! 2 high, 3 ultra; `--fx <preset>` on the command line), and per-feature
//! overrides `r_fx_<feature>` (-1 follows the preset, 0 off, 1 on). With the
//! preset off every shader takes the paths it took before the overhaul, so
//! the picture is the faithful one. The settings resolve into [`FxSettings`]
//! once a frame, which the render world gets a copy of.
//!
//! `r_fx_save` writes the `r_fx_*` values to `fx.cfg` in our own data folder
//! (next to the saves, never in the game's install), which runs at startup.

pub mod atmosphere;
pub mod lights;
pub mod material;
pub mod portal_light;
pub mod postfx;
pub mod reflections;
pub mod screen;
pub mod shadows;

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::render::RenderApp;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};

use portal2_core::console::{CommandArgs, Console, ConsoleSet};

/// Feature bits of [`FxSettings::features`], shared with the shaders
/// (`shaders/fx.wesl`).
pub mod features {
    /// Light from portals, lasers and (specular only) the map's fixtures.
    pub const LIGHTS: u32 = 1;
    /// Shadow maps for the strongest of those lights.
    pub const SHADOWS: u32 = 1 << 1;
    /// Screen-space ambient occlusion.
    pub const AO: u32 = 1 << 2;
    /// Screen-space reflections.
    pub const SSR: u32 = 1 << 3;
    /// Procedural wet patches on eligible surfaces.
    pub const WETNESS: u32 = 1 << 4;
    /// Distance haze in the materials.
    pub const FOG: u32 = 1 << 5;
    /// Light scattered in the air (beams and shafts).
    pub const VOLUMETRICS: u32 = 1 << 6;
    pub const BLOOM: u32 = 1 << 7;
    /// Tone curve and colour grade.
    pub const GRADE: u32 = 1 << 8;
    /// HDR self-illumination for signs, indicators and fixtures.
    pub const EMISSIVE: u32 = 1 << 9;
    /// Wrapped and transmitted light on foliage.
    pub const FOLIAGE: u32 = 1 << 10;
    /// Glossy reflections from the map's cube maps on every surface.
    pub const REFLECTIONS: u32 = 1 << 11;
    /// Wind sway, light flicker and dust.
    pub const PROCEDURAL: u32 = 1 << 12;
}

/// Override cvars and the feature each controls.
pub const FEATURE_CVARS: &[(&str, u32)] = &[
    ("r_fx_lights", features::LIGHTS),
    ("r_fx_shadows", features::SHADOWS),
    ("r_fx_ao", features::AO),
    ("r_fx_ssr", features::SSR),
    ("r_fx_wetness", features::WETNESS),
    ("r_fx_fog", features::FOG),
    ("r_fx_volumetrics", features::VOLUMETRICS),
    ("r_fx_bloom", features::BLOOM),
    ("r_fx_grade", features::GRADE),
    ("r_fx_emissive", features::EMISSIVE),
    ("r_fx_foliage", features::FOLIAGE),
    ("r_fx_reflections", features::REFLECTIONS),
    ("r_fx_procedural", features::PROCEDURAL),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FxPreset {
    Off,
    Medium,
    #[default]
    High,
    Ultra,
}

impl FxPreset {
    pub fn from_index(i: i32) -> Self {
        match i {
            i32::MIN..=0 => Self::Off,
            1 => Self::Medium,
            2 => Self::High,
            _ => Self::Ultra,
        }
    }

    /// `off`, `medium`, `high`, `ultra` or 0-3.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "legacy" => Some(Self::Off),
            "medium" | "med" => Some(Self::Medium),
            "high" => Some(Self::High),
            "ultra" => Some(Self::Ultra),
            other => other.parse::<i32>().ok().map(Self::from_index),
        }
    }
}

/// What the overhaul draws this frame.
#[derive(Resource, Clone, Debug, PartialEq, ExtractResource)]
#[extract_app(RenderApp)]
pub struct FxSettings {
    pub preset: FxPreset,
    /// [`features`] bits.
    pub features: u32,
    /// Most lights handed to the shaders at once.
    pub light_budget: usize,
    /// Shadow maps for overhaul lights, and their size.
    pub shadow_slots: usize,
    pub shadow_size: u32,
    pub bloom_intensity: f32,
    /// Multiplier on self-illumination.
    pub emissive_boost: f32,
    /// How much of an eligible surface turns wet (0..1).
    pub wetness: f32,
    /// Distance haze per 1000 units.
    pub haze_density: f32,
    pub volumetric_steps: u32,
    pub volumetric_density: f32,
    pub ao_strength: f32,
}

impl Default for FxSettings {
    fn default() -> Self {
        Self::for_preset(FxPreset::default())
    }
}

impl FxSettings {
    pub fn for_preset(preset: FxPreset) -> Self {
        use features::*;
        let base = Self {
            preset,
            features: 0,
            light_budget: 0,
            shadow_slots: 0,
            shadow_size: 1024,
            bloom_intensity: 0.0,
            emissive_boost: 1.0,
            wetness: 0.0,
            haze_density: 0.0,
            volumetric_steps: 0,
            volumetric_density: 0.0,
            ao_strength: 0.0,
        };
        let medium = LIGHTS | WETNESS | FOG | BLOOM | GRADE | EMISSIVE | FOLIAGE | REFLECTIONS | PROCEDURAL;
        match preset {
            FxPreset::Off => base,
            FxPreset::Medium => Self {
                features: medium,
                light_budget: 16,
                bloom_intensity: 0.035,
                emissive_boost: 1.6,
                wetness: 0.8,
                haze_density: 0.04,
                ..base
            },
            FxPreset::High => Self {
                features: medium | SHADOWS | AO | VOLUMETRICS,
                light_budget: 32,
                shadow_slots: 2,
                shadow_size: 1024,
                bloom_intensity: 0.05,
                emissive_boost: 2.0,
                wetness: 1.0,
                haze_density: 0.05,
                volumetric_steps: 24,
                volumetric_density: 1.0,
                ao_strength: 0.35,
                ..base
            },
            FxPreset::Ultra => Self {
                features: medium | SHADOWS | AO | VOLUMETRICS | SSR,
                light_budget: 64,
                shadow_slots: 4,
                shadow_size: 2048,
                bloom_intensity: 0.055,
                emissive_boost: 2.0,
                wetness: 1.0,
                haze_density: 0.05,
                volumetric_steps: 48,
                volumetric_density: 1.0,
                ao_strength: 0.4,
                ..base
            },
        }
    }

    pub fn has(&self, feature: u32) -> bool {
        self.features & feature != 0
    }

    pub fn enabled(&self) -> bool {
        self.features != 0
    }

    /// The preset's settings with the console's overrides applied.
    pub fn resolve(preset: FxPreset, overrides: impl Iterator<Item = (u32, i32)>) -> Self {
        let mut s = Self::for_preset(preset);
        for (feature, value) in overrides {
            match value {
                0 => s.features &= !feature,
                v if v > 0 => s.features |= feature,
                _ => {}
            }
        }
        // Lights the preset leaves without a budget still get a few.
        if s.has(features::LIGHTS) && s.light_budget == 0 {
            s.light_budget = 16;
        }
        if s.has(features::SHADOWS) && s.shadow_slots == 0 {
            s.shadow_slots = 2;
        }
        if s.has(features::BLOOM) && s.bloom_intensity == 0.0 {
            s.bloom_intensity = 0.04;
        }
        if s.has(features::EMISSIVE) && s.emissive_boost == 1.0 {
            s.emissive_boost = 1.6;
        }
        if s.has(features::WETNESS) && s.wetness == 0.0 {
            s.wetness = 0.8;
        }
        if s.has(features::FOG) && s.haze_density == 0.0 {
            s.haze_density = 0.04;
        }
        if s.has(features::VOLUMETRICS) && s.volumetric_steps == 0 {
            s.volumetric_steps = 24;
            s.volumetric_density = 1.0;
        }
        if s.has(features::AO) && s.ao_strength == 0.0 {
            s.ao_strength = 0.35;
        }
        s
    }
}

/// Main-camera depth and the like are copied in the main camera's passes;
/// these sets order the overhaul's passes on the interface camera around the
/// auto-exposure measurement (`tonemap::ExposureMeasureSet`).
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FxSceneSet;

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FxGradeSet;

pub struct VisualFxPlugin;

impl Plugin for VisualFxPlugin {
    fn build(&self, app: &mut App) {
        {
            let mut console = app.world_mut().resource_mut::<Console>();
            let archive = portal2_core::console::cvars::flags::ARCHIVE;
            console.register_cvar("r_fx_preset", "2", archive);
            for (name, _) in FEATURE_CVARS {
                console.register_cvar(name, "-1", archive);
            }
            console.register_command("r_fx_save", save_settings);
        }
        app.init_resource::<FxSettings>()
            .add_plugins(ExtractResourcePlugin::<FxSettings>::default())
            .add_systems(Startup, run_settings_file)
            .add_systems(PreUpdate, update_settings.after(ConsoleSet))
            .add_plugins((
                material::FxMaterialPlugin,
                lights::FxLightsPlugin,
                portal_light::PortalLightingPlugin,
                shadows::FxShadowsPlugin,
                screen::FxScreenPlugin,
                reflections::FxReflectionsPlugin,
                atmosphere::FxAtmospherePlugin,
                postfx::PostFxPlugin,
            ));
    }
}

fn update_settings(console: Res<Console>, mut settings: ResMut<FxSettings>) {
    let preset = FxPreset::from_index(console.i32("r_fx_preset"));
    let overrides = FEATURE_CVARS.iter().map(|(name, feature)| (*feature, console.cvar(name).map_or(-1, |c| c.i32())));
    settings.set_if_neq(FxSettings::resolve(preset, overrides));
}

/// `fx.cfg` in our data folder (beside the saves).
fn settings_path() -> Option<PathBuf> {
    crate::save::save_dir().parent().map(|p| p.join("fx.cfg"))
}

/// Runs `fx.cfg`, then the command line's `--fx`.
fn run_settings_file(mut console: ResMut<Console>, args: Res<portal2_core::args::Args>) {
    if let Some(text) = settings_path().and_then(|p| std::fs::read_to_string(p).ok()) {
        console.add_text(&text);
    }
    if let Some(preset) = args.fx.as_deref().and_then(FxPreset::parse) {
        console.add_text(&format!("r_fx_preset {}", preset as i32));
    }
}

fn save_settings(world: &mut World, _args: &CommandArgs) {
    let mut text = String::from("// Visual overhaul settings, written by r_fx_save.\n");
    {
        let console = world.resource::<Console>();
        text.push_str(&format!("r_fx_preset {}\n", console.str("r_fx_preset")));
        for (name, _) in FEATURE_CVARS {
            text.push_str(&format!("{name} {}\n", console.str(name)));
        }
    }
    let result = settings_path().ok_or_else(|| "no data folder".to_owned()).and_then(|path| {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, text).map_err(|e| e.to_string()).map(|_| path)
    });
    let line = match result {
        Ok(path) => format!("Wrote {}", path.display()),
        Err(e) => format!("r_fx_save failed: {e}"),
    };
    world.resource_mut::<Console>().print(line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_and_overrides() {
        assert!(!FxSettings::for_preset(FxPreset::Off).enabled());
        let high = FxSettings::for_preset(FxPreset::High);
        assert!(high.has(features::LIGHTS) && high.has(features::SHADOWS) && !high.has(features::SSR));
        assert!(FxSettings::for_preset(FxPreset::Ultra).has(features::SSR));
        // An override turns a single feature off, or on over the off preset.
        let no_bloom = FxSettings::resolve(FxPreset::High, [(features::BLOOM, 0)].into_iter());
        assert!(!no_bloom.has(features::BLOOM) && no_bloom.has(features::LIGHTS));
        let lights_only = FxSettings::resolve(FxPreset::Off, [(features::LIGHTS, 1)].into_iter());
        assert_eq!(lights_only.features, features::LIGHTS);
        assert!(lights_only.light_budget > 0);
        // -1 follows the preset.
        assert_eq!(FxSettings::resolve(FxPreset::Medium, [(features::AO, -1)].into_iter()), FxSettings::for_preset(FxPreset::Medium));
        assert_eq!(FxPreset::parse("ultra"), Some(FxPreset::Ultra));
        assert_eq!(FxPreset::parse("0"), Some(FxPreset::Off));
        assert_eq!(FxPreset::parse("9"), Some(FxPreset::Ultra));
        assert_eq!(FxPreset::parse("bogus"), None);
    }
}
