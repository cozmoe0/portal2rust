//! The overhaul's portals (`r_fx_portals`): procedural fire and smoke in
//! place of the game's noise texture and colour gradient (`portal_fx.wesl`).
//!
//! Each portal's three materials (the opening in the window, in the outer
//! view and past the last level) switch to the fire's shader variant, and
//! its three surfaces to the quad that reaches past the portal's rectangle,
//! whenever the feature turns on or off or a portal appears. Materials are
//! only written when they change (each write prepares them again).
//!
//! With the fire's heat (`r_fx_portal_heat`) the materials also refract the
//! view through near the rim, and the portal's heat haze surfaces (the wall
//! around the rim shimmering, `portal_haze.wesl`) show.

use bevy::prelude::*;

use super::{FxSettings, features};
use crate::portals::{Portal, PortalHazeMaterial, PortalMaterial};

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
    mut haze_materials: ResMut<Assets<PortalHazeMaterial>>,
    mut meshes: Query<&mut Mesh3d>,
    mut visibility: Query<&mut Visibility>,
) {
    let on = settings.has(features::PORTALS);
    let heat = if on && settings.has(features::PORTAL_HEAT) { 1.0 } else { 0.0 };
    for p in &portals {
        if !settings.is_changed() && !p.is_added() {
            continue;
        }
        for handle in [&p.material, &p.inner_material, &p.end_material] {
            if materials.get(handle).is_some_and(|m| m.fx != on || m.params.glow.w != heat)
                && let Some(mut m) = materials.get_mut(handle)
            {
                m.fx = on;
                m.params.glow.w = heat;
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
        if haze_materials.get(&p.haze_material).is_some_and(|m| m.params.state.y != heat)
            && let Some(mut m) = haze_materials.get_mut(&p.haze_material)
        {
            m.params.state.y = heat;
        }
        let shown = if heat > 0.0 { Visibility::Inherited } else { Visibility::Hidden };
        for haze in p.haze {
            if let Ok(mut v) = visibility.get_mut(haze) {
                v.set_if_neq(shown);
            }
        }
    }
}
