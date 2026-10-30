//! What world and model materials share with the visual overhaul (the game
//! crate's `visuals::fx`): a storage buffer with the overhaul's switches and
//! lights, its shadow maps and the map's reflection probes.
//!
//! Materials carry an `fx` flag that selects the shader variant with the
//! overhaul's code (`FX` shader def). With it off they compile exactly the
//! shaders they had before, so turning the overhaul off restores the
//! faithful picture bit for bit. New materials take the current state from
//! [`FX_ENABLED`].

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::prelude::*;
use bevy::render::storage::ShaderBuffer;

pub const FX_SHADER: &str = "embedded://portal2_render/shaders/fx.wesl";

/// Floats in the storage buffer before the lights (see `shaders/fx.wesl`).
pub const HEADER_FLOATS: usize = 128;
/// Floats per light: five vectors.
pub const LIGHT_FLOATS: usize = 20;
pub const MAX_LIGHTS: usize = 64;
pub const BUFFER_FLOATS: usize = HEADER_FLOATS + MAX_LIGHTS * LIGHT_FLOATS;
/// Shadow maps for overhaul lights.
pub const MAX_SHADOWS: usize = 4;

/// Handles every world and model material binds. Set once at startup;
/// materials built before that (tests) bind nothing usable.
#[derive(Clone, Debug)]
pub struct FxHandles {
    pub buffer: Handle<ShaderBuffer>,
}

pub static FX: OnceLock<FxHandles> = OnceLock::new();

/// Whether new materials use the overhaul's shader variant.
pub static FX_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn fx_enabled() -> bool {
    FX_ENABLED.load(Ordering::Relaxed)
}

pub fn set_fx_enabled(on: bool) {
    FX_ENABLED.store(on, Ordering::Relaxed);
}

pub(crate) fn fx_buffer() -> Handle<ShaderBuffer> {
    FX.get().map(|h| h.buffer.clone()).unwrap_or_default()
}
