//! The locator (`CLocatorPanel`): draws the lessons being taught. Icons are
//! `0.04 x screen width` tall times `locator_icon_min_size_non_ss`, pulse
//! up towards `locator_icon_max_size_non_ss` for `locator_pulse_time`
//! after appearing, and fade in and out over `locator_fade_time`.
//!
//! A lesson about the player is static: icon, 5 pixels, then the caption,
//! centred across the screen, the first at two thirds of the screen height
//! and each further one an icon and a quarter lower; each slides there from
//! the screen centre. Other lessons draw on their target with the caption
//! beside it (only within 720 units, beyond which the icon is half size),
//! are hidden while a wall is in the way, and sit on a ring around the
//! screen centre while the target is off screen. A `use_binding` icon is
//! the key cap of the key bound to the lesson's command (several commands:
//! one after another every 1.25 seconds) with the key's name on it.

use std::collections::HashMap;
use std::f32::consts::PI;

use bevy::prelude::*;
use source_formats::bsp::contents;

use super::GameInstructor;
use portal2_core::clock::GameClock;
use portal2_core::console::Console;
use portal2_core::entity::EntityRegistry;
use portal2_core::vfs::GameFs;
use portal2_core::view::MainCamera;
use portal2_game::screen::Localization;
use portal2_ui::vgui::surface::{FontId, ImageRef};
use portal2_ui::vgui::{Schemes, Surface, VguiPaint, layer};
use portal2_world::collision::Physics;

pub fn build(app: &mut App) {
    app.init_resource::<LocatorState>().add_systems(PostUpdate, paint.in_set(VguiPaint));
}

/// A rectangle of a sheet from `scripts/instructor_textures.txt`.
struct HudTexture {
    file: String,
    rect: [u32; 4],
}

#[derive(Default)]
struct TargetState {
    /// Centre height of a static lesson, sliding to its place.
    y: Option<f32>,
    occluded: bool,
    next_trace: f64,
}

#[derive(Resource, Default)]
struct LocatorState {
    textures: Option<HashMap<String, HudTexture>>,
    targets: HashMap<u64, TargetState>,
}

fn load_textures(fs: &GameFs) -> HashMap<String, HudTexture> {
    let mut out = HashMap::new();
    for file in ["scripts/instructor_textures.txt", "scripts/mod_textures.txt"] {
        let Some(text) = fs.read_string(file) else { continue };
        let doc = source_formats::kv::parse_document(&text);
        for root in &doc.roots {
            let Some(data) = root.get("TextureData") else { continue };
            for t in data.children().iter().filter(|c| c.is_block()) {
                let n = |k: &str| t.get_i32(k).unwrap_or(0).max(0) as u32;
                let Some(sheet) = t.get_str("file") else { continue };
                out.insert(
                    t.key.to_ascii_lowercase(),
                    HudTexture { file: sheet.to_owned(), rect: [n("x"), n("y"), n("width"), n("height")] },
                );
            }
        }
    }
    out
}

/// Source's spelling of a bind table key: letters and digits lower case,
/// named keys upper case.
fn key_name(key: &str) -> String {
    if key.chars().count() == 1 { key.to_ascii_lowercase() } else { key.to_ascii_uppercase() }
}

/// The engine's key order for "first key bound to": keyboard (digits,
/// letters, keypad, the rest), then mouse, then gamepad.
fn key_order(key: &str) -> (u8, String) {
    let k = key.to_ascii_lowercase();
    let class = if k.len() == 1 && k.as_bytes()[0].is_ascii_digit() {
        0
    } else if k.len() == 1 && k.as_bytes()[0].is_ascii_alphabetic() {
        1
    } else if k.starts_with("kp_") {
        2
    } else if k.starts_with("mouse") || k.starts_with("mwheel") {
        4
    } else if k.contains("button") || k.contains("trigger") || k.contains("shoulder") || k.starts_with("joy") || k.starts_with("pov") {
        5
    } else {
        3
    };
    (class, k)
}

/// Key bound to `command` (a leading `+` may be left off).
fn lookup_binding(console: &Console, command: &str) -> Option<String> {
    let want = command.trim().trim_start_matches('+').to_ascii_lowercase();
    if want.is_empty() {
        return None;
    }
    console
        .binds
        .iter()
        .filter(|(_, cmd)| cmd.trim().trim_start_matches('+').eq_ignore_ascii_case(&want))
        .map(|(k, _)| k.as_str())
        .min_by_key(|k| key_order(k))
        .map(key_name)
}

/// Icon and label for a bound key.
fn key_icon(key: &str) -> (&'static str, Option<String>) {
    let upper = key.to_ascii_uppercase();
    match upper.as_str() {
        "MOUSE1" => ("icon_mouseleft", None),
        "MOUSE2" => ("icon_mouseright", None),
        "MOUSE3" => ("icon_mousethree", None),
        "MWHEELUP" => ("icon_mousewheel_up", None),
        "MWHEELDOWN" => ("icon_mousewheel_down", None),
        "UPARROW" => ("icon_key_up", None),
        "LEFTARROW" => ("icon_key_left", None),
        "DOWNARROW" => ("icon_key_down", None),
        "RIGHTARROW" => ("icon_key_right", None),
        _ => {
            let narrow = ["SEMICOLON", "INS", "DEL", "HOME", "END", "PGUP", "PGDN", "PAUSE", "F10", "F11", "F12"];
            let icon = if narrow.contains(&upper.as_str()) || upper.len() <= 2 { "icon_key_generic" } else { "icon_key_wide" };
            let label = match upper.as_str() {
                "SEMICOLON" => ";".to_owned(),
                "ESCAPE" => "ESC".to_owned(),
                _ if key.len() == 1 => upper,
                _ => key.to_owned(),
            };
            (icon, Some(label))
        }
    }
}

struct Fonts {
    title: FontId,
    keys: FontId,
    keys_small: FontId,
}

/// One lesson to draw this frame.
struct Item<'a> {
    lesson: &'a super::OpenLesson,
    alpha: f32,
    /// When it appeared (pulse and key cycling start).
    since: f64,
}

#[allow(clippy::too_many_arguments)]
fn paint(
    gi: Res<GameInstructor>,
    console: Res<Console>,
    clock: Res<GameClock>,
    time: Res<Time>,
    ui: Res<crate::frontend::gameui::GameUi>,
    map: Option<Res<portal2_world::CurrentMap>>,
    schemes: Res<Schemes>,
    localization: Option<Res<Localization>>,
    fs: Res<GameFs>,
    registry: Res<EntityRegistry>,
    transforms: Query<&GlobalTransform>,
    cameras: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    physics: Physics,
    mut surface: ResMut<Surface>,
    mut state: ResMut<LocatorState>,
) {
    let now = clock.curtime;
    let fade = console.f32("locator_fade_time").max(0.001) as f64;
    // Lessons shown, fading in, or fading out after stopping or closing.
    let mut items: Vec<Item> = Vec::new();
    for l in &gi.open {
        match (l.started, l.stopped) {
            (Some(s), _) => items.push(Item { lesson: l, alpha: ((now - s) / fade).clamp(0.0, 1.0) as f32, since: s }),
            (None, Some(t)) if now - t < fade => items.push(Item { lesson: l, alpha: (1.0 - (now - t) / fade) as f32, since: t }),
            _ => {}
        }
    }
    for (l, at) in &gi.closing {
        items.push(Item { lesson: l, alpha: (1.0 - (now - at) / fade).clamp(0.0, 1.0) as f32, since: l.started.unwrap_or(*at) });
    }
    let live: Vec<u64> = items.iter().map(|i| i.lesson.id).collect();
    state.targets.retain(|id, _| live.contains(id));
    if items.is_empty() || map.is_none() || ui.active() || !portal2_ui::hud::visible(&console) || !console.bool("gameinstructor_enable") {
        return;
    }
    if state.textures.is_none() {
        state.textures = Some(load_textures(&fs));
    }
    let (w, h) = (surface.wide, surface.tall);
    let font = |name: &str, s: &mut Surface| schemes.basemodui.font_spec(name, h, true).map(|f| s.font(f));
    let Some(title) = font("InstructorTitle", &mut surface) else { return };
    let keys = font("InstructorKeyBindings", &mut surface).unwrap_or(title);
    let keys_small = font("InstructorKeyBindingsSmall", &mut surface).unwrap_or(keys);
    let fonts = Fonts { title, keys, keys_small };
    surface.set_layer(layer::HUD);

    let min = console.f32("locator_icon_min_size_non_ss");
    let max = console.f32("locator_icon_max_size_non_ss");
    let pulse_time = console.f32("locator_pulse_time") as f64;
    let lerp_speed = console.f32("locator_lerp_speed");
    let dt = time.delta_secs();
    let base = (w as f32 * 0.04) as i32;
    let camera = cameras.single().ok();
    let eye = camera.map(|(_, t)| t.translation());
    let mut static_y = (h / 2 + h / 6) as f32;

    for item in &items {
        let l = item.lesson;
        let v = &l.vars;
        let is_static = v.icon_target.is_some() && v.icon_target == registry.player;
        // Icon (a key cap for `use_binding`) and its label.
        let (icon_name, label) = if v.onscreen_icon.eq_ignore_ascii_case("use_binding") {
            let bound: Vec<String> = v.binding.split(';').filter_map(|c| lookup_binding(&console, c)).collect();
            // Nothing to press: the lesson is not drawn.
            if bound.is_empty() {
                continue;
            }
            let k = &bound[((now - item.since).max(0.0) / 1.25) as usize % bound.len()];
            let (icon, label) = key_icon(k);
            (icon.to_owned(), label)
        } else {
            (v.onscreen_icon.to_ascii_lowercase(), None)
        };
        let Some(tex) = state.textures.as_ref().and_then(|t| t.get(&icon_name)) else { continue };
        let (file, rect) = (tex.file.clone(), tex.rect);
        let Some(sheet) = surface.image(&file) else { continue };
        // Size with the pulse after appearing.
        let aspect = rect[2] as f32 / rect[3].max(1) as f32;
        let (mut wide, mut tall) = ((base as f32 * aspect) as i32, base);
        let age = now - item.since;
        let mut scale = if age < pulse_time { (PI * age as f32).sin().abs() * (max - min) + min } else { min };
        if wide > tall {
            scale = ((tall as f32 * scale - tall as f32) as i32 + wide) as f32 / wide as f32;
        }
        wide = (wide as f32 * scale) as i32;
        tall = (tall as f32 * scale) as i32;
        let alpha = (item.alpha * 255.0) as u8;
        let caption = localization.as_ref().map(|loc| loc.text(&v.caption)).unwrap_or_else(|| v.caption.clone());
        let color = v.caption_color;

        if is_static {
            let text_w = surface.text_width(fonts.title, &caption);
            let target = state.targets.entry(l.id).or_default();
            let y = target.y.get_or_insert((h / 2) as f32);
            let dy = static_y - *y;
            let step = (lerp_speed * dt * dy.abs()).max(3.0).min(dy.abs());
            *y += step * dy.signum();
            let cy = *y as i32;
            let x = ((w / 2) as f32 - (text_w + wide + 5) as f32 * 0.5) as i32;
            draw_icon(&mut surface, &sheet, rect, x, cy - tall / 2, wide, tall, alpha);
            if let Some(label) = &label {
                draw_label(&mut surface, &fonts, label, x + wide / 2, cy - tall / 2 + tall / 2, alpha);
            }
            draw_caption(&mut surface, fonts.title, &caption, x + wide + 5, cy, color, alpha);
            static_y += (tall + tall / 4) as f32;
            continue;
        }

        // A lesson on something in the world.
        let (Some((cam, cam_t)), Some(eye), Some(target)) = (camera, eye, v.icon_target.and_then(|t| transforms.get(t).ok())) else {
            continue;
        };
        let pos = target.translation() + cam_t.up() * v.icon_offset;
        let distance = pos.distance(eye);
        let target_state = state.targets.entry(l.id).or_default();
        if now >= target_state.next_trace {
            target_state.next_trace = now + 0.25;
            let ignore: Vec<Entity> = registry.player.into_iter().chain(v.icon_target).collect();
            target_state.occluded =
                physics.trace(eye, pos, Vec3::ZERO, Vec3::ZERO, contents::SOLID | contents::MOVEABLE, &ignore).fraction < 1.0;
        }
        if target_state.occluded && !v.force_caption {
            continue;
        }
        let px_scale = cam.logical_viewport_size().map(|s| w as f32 / s.x.max(1.0)).unwrap_or(1.0);
        let screen = cam.world_to_viewport(cam_t, pos).ok().map(|p| p * px_scale);
        let margin = w as f32 * 0.075;
        let on_screen = screen.filter(|p| p.x >= margin && p.x <= w as f32 - margin && p.y >= margin && p.y <= h as f32 - margin);
        if distance > 720.0 {
            wide /= 2;
            tall /= 2;
        }
        match on_screen {
            Some(p) => {
                let (x, y) = (p.x as i32 - wide / 2, p.y as i32 - tall / 2);
                draw_icon(&mut surface, &sheet, rect, x, y, wide, tall, alpha);
                if let Some(label) = &label {
                    draw_label(&mut surface, &fonts, label, x + wide / 2, y + tall / 2, alpha);
                }
                if distance <= 720.0 || v.force_caption {
                    let text_x = (base as f32 * aspect * 0.5 + (p.x as i32 + 5) as f32) as i32;
                    draw_caption(&mut surface, fonts.title, &caption, text_x, p.y as i32, color, alpha);
                }
            }
            None if !v.no_offscreen => {
                // On a ring around the screen centre, towards the target.
                let to = (pos - eye).normalize_or_zero();
                let dir = Vec2::new(to.dot(*cam_t.right()), -to.dot(*cam_t.up())).normalize_or(Vec2::Y);
                let radius = h as f32 * 100.0 / 480.0;
                let c = Vec2::new((w / 2) as f32, (h / 2) as f32) + dir * radius;
                let off_name = v.offscreen_icon.to_ascii_lowercase();
                let (file, rect) = match state.textures.as_ref().and_then(|t| t.get(&off_name)) {
                    Some(t) => (t.file.clone(), t.rect),
                    None => (file, rect),
                };
                let Some(sheet) = surface.image(&file) else { continue };
                draw_icon(&mut surface, &sheet, rect, c.x as i32 - wide / 2, c.y as i32 - tall / 2, wide, tall, alpha);
            }
            None => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_icon(surface: &mut Surface, sheet: &ImageRef, rect: [u32; 4], x: i32, y: i32, wide: i32, tall: i32, alpha: u8) {
    let (sw, sh) = (sheet.width.max(1) as f32, sheet.height.max(1) as f32);
    let uv = [rect[0] as f32 / sw, rect[1] as f32 / sh, (rect[0] + rect[2]) as f32 / sw, (rect[1] + rect[3]) as f32 / sh];
    surface.draw_image_uv(sheet, x as f32, y as f32, (x + wide) as f32, (y + tall) as f32, uv, [255, 255, 255, alpha]);
}

/// The key's name on its cap, black, centred one pixel up and left.
fn draw_label(surface: &mut Surface, fonts: &Fonts, label: &str, cx: i32, cy: i32, alpha: u8) {
    let font = if label.chars().count() <= 5 { fonts.keys } else { fonts.keys_small };
    let tw = surface.text_width(font, label);
    let tall = surface.font_tall(font);
    surface.draw_text(font, cx - tw / 2 - 1, cy - tall / 2 - 1, label, [0, 0, 0, alpha]);
}

/// The caption, vertically centred on `cy`: a black copy one pixel down
/// and right under the coloured text.
fn draw_caption(surface: &mut Surface, font: FontId, text: &str, x: i32, cy: i32, color: [u8; 3], alpha: u8) {
    let y = cy - surface.font_tall(font) / 2;
    surface.draw_text(font, x, y, text, [0, 0, 0, alpha]);
    surface.draw_text(font, x - 1, y - 1, text, [color[0], color[1], color[2], alpha]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_caps_follow_the_key_names() {
        assert_eq!(key_icon("e"), ("icon_key_generic", Some("E".to_owned())));
        assert_eq!(key_icon("SPACE"), ("icon_key_wide", Some("SPACE".to_owned())));
        assert_eq!(key_icon("F11"), ("icon_key_generic", Some("F11".to_owned())));
        assert_eq!(key_icon("MOUSE1"), ("icon_mouseleft", None));
        assert_eq!(key_icon("SEMICOLON"), ("icon_key_generic", Some(";".to_owned())));
    }

    #[test]
    fn keyboard_keys_come_before_gamepad_buttons() {
        let mut keys = vec!["a_button", "space", "w"];
        keys.sort_by_key(|k| key_order(k));
        assert_eq!(keys, ["w", "space", "a_button"]);
    }
}
