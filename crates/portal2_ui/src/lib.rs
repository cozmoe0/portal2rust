//! The game's interface toolkit, rebuilt from the original's behaviour:
//! VGUI schemes, `.res` layouts, fonts rasterized with GDI the way the
//! game does, and a painter that blends in linear light into the HDR view
//! ([`vgui`]); Bink movie playback for the menus and in-world screens
//! ([`video`]); the close captions panel ([`captions`]); and the HUD layout
//! the HUD elements are placed with ([`hud`]).

pub mod captions;
pub mod hud;
pub mod vgui;
pub mod video;
