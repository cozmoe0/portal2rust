//! Pieces of BaseModUI dialogs: the tiled background with its title tab,
//! the footer buttons under a dialog, and the chapter list of New Game.

use portal2_ui::vgui::Surface;
use portal2_ui::vgui::layout::Rect;
use portal2_ui::vgui::scheme::{Scheme, proportional};
use portal2_ui::vgui::surface::Rgba;

/// Tile size in pixels.
pub fn tile_size(scheme: &Scheme, tall: i32) -> (i32, i32) {
    let tw = proportional(scheme.setting_i32("Dialog.TileWidth").unwrap_or(50), tall).max(1);
    let th = proportional(scheme.setting_i32("Dialog.TileHeight").unwrap_or(50), tall).max(1);
    (tw, th)
}

/// The tiles under a dialog: an optional title tab `tab` tiles wide above
/// the body (`body` is the body's rectangle, whole tiles).
pub fn paint_tiles(surface: &mut Surface, scheme: &Scheme, body: Rect, tab: i32, alt: bool) {
    let (tw, th) = tile_size(scheme, surface.tall);
    let (cols, rows) = (body.w / tw, body.h / th);
    let tab = tab.min(cols);
    let dir = if alt { "vgui/menu_tiles/alt/" } else { "vgui/menu_tiles/" };
    let draw = |surface: &mut Surface, col: i32, row: i32, name: &str| {
        if let Some(img) = surface.image(&format!("{dir}UI_tile_128_{name}")) {
            let (x, y) = (body.x + col * tw, body.y + row * th);
            surface.draw_image(&img, x, y, x + tw, y + th, [255, 255, 255, 255]);
        }
    };
    if tab > 0 {
        for col in 0..tab {
            let name = match (col == 0, col == tab - 1) {
                (true, _) => "crnr_topL",
                (_, true) => "crnr_topR",
                _ => "top",
            };
            draw(surface, col, -1, name);
        }
    }
    for row in 0..rows {
        for col in 0..cols {
            let (top, bottom, left, right) = (row == 0, row == rows - 1, col == 0, col == cols - 1);
            // Under the tab the body's top row continues the tab.
            let covered = top && col < tab;
            let name = match (top && !covered, bottom, left, right) {
                (true, _, true, _) => "crnr_topL",
                (true, _, _, true) => "crnr_topR",
                (_, true, true, _) => "crnr_bottomL",
                (_, true, _, true) => "crnr_bottomR",
                (true, _, _, _) => "top",
                (_, true, _, _) => "bottom",
                (_, _, true, _) => "left",
                (_, _, _, true) => "right",
                _ => "interior1",
            };
            draw(surface, col, row, name);
        }
    }
}

/// Tiles a title of this width needs (text plus its offset on both sides).
pub fn tab_tiles(surface: &mut Surface, scheme: &Scheme, title: &str) -> i32 {
    if title.is_empty() {
        return 0;
    }
    let tall = surface.tall;
    let (tw, _) = tile_size(scheme, tall);
    let Some(spec) = scheme.font_spec(scheme.setting("Dialog.TitleFont").unwrap_or("DialogTitle"), tall, true) else {
        return 0;
    };
    let font = surface.font(spec);
    let w = surface.text_width(font, title) + 2 * proportional(scheme.setting_i32("Dialog.TitleOffsetX").unwrap_or(10), tall);
    (w + tw - 1) / tw
}

pub fn paint_title(surface: &mut Surface, scheme: &Scheme, body: Rect, title: &str, alt: bool) {
    let tall = surface.tall;
    let (_, th) = tile_size(scheme, tall);
    let Some(spec) = scheme.font_spec(scheme.setting("Dialog.TitleFont").unwrap_or("DialogTitle"), tall, true) else { return };
    let font = surface.font(spec);
    let key = if alt { "Dialog.TitleColorAlt" } else { "Dialog.TitleColor" };
    let color = scheme.setting_color(key).unwrap_or([0, 0, 0, 255]);
    let x = body.x + proportional(scheme.setting_i32("Dialog.TitleOffsetX").unwrap_or(10), tall);
    let y = body.y - th + proportional(scheme.setting_i32("Dialog.TitleOffsetY").unwrap_or(9), tall);
    surface.draw_text(font, x, y, title, color);
}

/// Footer buttons under a dialog (`FooterPanel.*`): boxed labels from the
/// dialog's left edge, `ButtonGapX` apart. Returns their rectangles.
pub fn paint_footer(surface: &mut Surface, scheme: &Scheme, body: Rect, labels: &[&str], hover: Option<usize>) -> Vec<Rect> {
    let tall = surface.tall;
    let font_name = scheme.setting("FooterPanel.TextFont").unwrap_or("ButtonText_NC");
    let Some(spec) = scheme.font_spec(font_name, tall, true) else { return Vec::new() };
    let font = surface.font(spec);
    let gap = proportional(scheme.setting_i32("FooterPanel.ButtonGapX").unwrap_or(12), tall);
    let pad = scheme.setting_i32("FooterPanel.ButtonPaddingX").unwrap_or(20);
    let y = body.bottom() + proportional(scheme.setting_i32("FooterPanel.OffsetY").unwrap_or(12), tall);
    let h = proportional(14, tall);
    let border = scheme.setting_color("FooterPanel.BorderColor").unwrap_or([130, 130, 130, 255]);
    let armed = scheme.setting_color("FooterPanel.BorderArmedColor").unwrap_or([180, 180, 180, 255]);
    let text_color = scheme.setting_color("FooterPanel.TextColor").unwrap_or([0, 0, 0, 255]);
    let mut x = body.x;
    let mut rects = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        let text = label.to_uppercase();
        let w = surface.text_width(font, &text) + 2 * pad;
        let r = Rect::new(x, y, w, h);
        let fill: Rgba = if hover == Some(i) { armed } else { [137, 137, 138, 255] };
        surface.fill(r.x, r.y, r.right(), r.bottom(), fill);
        surface.outline(r.x, r.y, r.right(), r.bottom(), border);
        let ty = r.y + (r.h - surface.font_tall(font)) / 2;
        surface.draw_text(font, r.x + pad, ty, &text, text_color);
        rects.push(r);
        x += w + gap;
    }
    rects
}
