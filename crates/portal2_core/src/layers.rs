//! Render layers (`RenderLayers` indices) that keep the views apart.

/// Portal surfaces seen by the player camera only.
pub const PORTAL_LAYER: usize = 1;
/// The first-person weapon.
pub const VIEWMODEL_LAYER: usize = 2;
/// Geometry of the 3D sky box, seen only by its camera.
pub const SKYBOX_LAYER: usize = 3;
/// The 2D sky quads.
pub const SKY2D_LAYER: usize = 4;
/// Portal surfaces seen inside a portal's view.
pub const PORTAL_INNER_LAYER: usize = 5;
/// Portal surfaces at the last level of views.
pub const PORTAL_END_LAYER: usize = 6;
/// The player's body, seen by the portal views only.
pub const PLAYER_BODY_LAYER: usize = 7;
