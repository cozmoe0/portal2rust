//! Drawing shared by panels.

use super::surface::{Rgba, Surface};

/// `Panel::DrawBox`: a filled box whose four corners are the rounded
/// `vgui/hud/800corner*` textures, drawn at their own size.
pub fn draw_box(surface: &mut Surface, x: i32, y: i32, w: i32, h: i32, color: Rgba) {
    if color[3] == 0 || w <= 0 || h <= 0 {
        return;
    }
    let corners = [
        surface.image("vgui/hud/800corner1"),
        surface.image("vgui/hud/800corner2"),
        surface.image("vgui/hud/800corner3"),
        surface.image("vgui/hud/800corner4"),
    ];
    let Some(first) = corners[0] else {
        surface.fill(x, y, x + w, y + h, color);
        return;
    };
    let (cw, ch) = (first.width as i32, first.height as i32);
    surface.fill(x + cw, y, x + w - cw, y + ch, color);
    surface.fill(x, y + ch, x + w, y + h - ch, color);
    surface.fill(x + cw, y + h - ch, x + w - cw, y + h, color);
    // Top left, top right, bottom right, bottom left.
    let spots = [(x, y), (x + w - cw, y), (x + w - cw, y + h - ch), (x, y + h - ch)];
    for (corner, (cx, cy)) in corners.iter().zip(spots) {
        match corner {
            Some(img) => surface.draw_image(img, cx, cy, cx + cw, cy + ch, color),
            None => surface.fill(cx, cy, cx + cw, cy + ch, color),
        }
    }
}
