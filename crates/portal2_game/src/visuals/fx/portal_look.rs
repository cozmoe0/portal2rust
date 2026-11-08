//! The overhaul's portals (`r_fx_portals`): procedural fire and smoke in
//! place of the game's noise texture and colour gradient (`portal_fx.wesl`).
//!
//! Each portal's three materials (the opening in the window, in the outer
//! view and past the last level) switch to the fire's shader variant, and
//! its three surfaces to the quad that reaches past the portal's rectangle,
//! whenever the feature turns on or off or a portal appears. Materials are
//! only written when they change (each write prepares them again).

use bevy::prelude::*;

use super::{FxSettings, features};
use crate::portals::{Portal, PortalMaterial};

pub struct PortalLookPlugin;

impl Plugin for PortalLookPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, sync_portal_look);
    }
}

fn sync_portal_look(
    settings: Res<FxSettings>,
    portals: Query<Ref<Portal>>,
    mut materials: ResMut<Assets<PortalMaterial>>,
    mut meshes: Query<&mut Mesh3d>,
) {
    let on = settings.has(features::PORTALS);
    for p in &portals {
        if !settings.is_changed() && !p.is_added() {
            continue;
        }
        for handle in [&p.material, &p.inner_material, &p.end_material] {
            if materials.get(handle).is_some_and(|m| m.fx != on)
                && let Some(mut m) = materials.get_mut(handle)
            {
                m.fx = on;
            }
        }
        let quad = if on { &p.fx_quad } else { &p.quad };
        for surface in p.surfaces {
            if let Ok(mut mesh) = meshes.get_mut(surface)
                && mesh.0 != *quad
            {
                mesh.0 = quad.clone();
            }
        }
    }
}
