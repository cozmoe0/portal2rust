//! VGUI images: `vgui/...` materials (or bare `.vtf` names) decoded to
//! plain RGBA for the interface painter.

use bevy::prelude::*;
use source_formats::fs::GameFileSystem;
use source_formats::vmt::Vmt;
use source_formats::vtf::Vtf;

/// Material path for an image name as panels give it: names are relative
/// to `materials/` (`vgui/foo`, `console/background01`), extensions optional.
pub fn material_path(name: &str) -> String {
    let n = name.trim_start_matches('/');
    let n = n.strip_prefix("materials/").unwrap_or(n);
    let n = n.strip_suffix(".vmt").unwrap_or(n);
    format!("materials/{n}.vmt")
}

/// Loads an image: (image, width, height, additive).
pub fn load(fs: &GameFileSystem, name: &str) -> Option<(Image, u32, u32, bool)> {
    let (vtf_path, additive, alpha_test) = if name.ends_with(".vtf") {
        (format!("materials/{}", name.trim_start_matches("materials/")), false, None)
    } else {
        let path = material_path(name);
        let text = fs.read_string(&path)?;
        let vmt = Vmt::parse(&text, &mut |p| fs.read_string(&format!("materials/{p}")).or_else(|| fs.read_string(p)))?;
        let alpha_test = vmt.get_bool("$alphatest").then(|| vmt.get_f32("$alphatestreference").unwrap_or(0.5));
        (vmt.texture_path("$basetexture")?, vmt.get_bool("$additive"), alpha_test)
    };
    let bytes = fs.read(&vtf_path)?;
    let vtf = match Vtf::parse(&bytes) {
        Ok(v) => v,
        Err(e) => {
            warn!("vgui: bad texture {vtf_path}: {e}");
            return None;
        }
    };
    let mut rgba = vtf.decode_rgba8(0, 0, 0);
    // Alpha-tested texels below the reference are not drawn at all.
    if let Some(reference) = alpha_test {
        let cut = (reference * 255.0) as u8;
        for px in rgba.as_chunks_mut::<4>().0 {
            if px[3] < cut {
                px[3] = 0;
            }
        }
    }
    let image = super::surface::rgba_image(vtf.width, vtf.height, rgba, false);
    Some((image, vtf.width, vtf.height, additive))
}
