//! The developer console window (`toggleconsole`, usually the ` key): the
//! console's output above an entry line, in the source scheme's colours and
//! console font. Enter runs the line, Up/Down walk the history, Escape or
//! the toggle key close it.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;

use portal2_core::console::Console;
use portal2_ui::vgui::{Schemes, Surface, VguiPaint, layer};

pub fn build(app: &mut App) {
    app.init_resource::<ConsoleUi>().add_systems(Update, input.before(super::input)).add_systems(PostUpdate, paint.in_set(VguiPaint));
    app.world_mut().resource_mut::<Console>().register_command("toggleconsole", |world, _| {
        let mut ui = world.resource_mut::<ConsoleUi>();
        ui.open = !ui.open;
        ui.ignore_next = true;
    });
}

#[derive(Resource, Default)]
pub struct ConsoleUi {
    pub open: bool,
    line: String,
    history: Vec<String>,
    position: Option<usize>,
    /// The key that opened the console also types a character; skip it.
    ignore_next: bool,
}

fn input(
    mut ui: ResMut<ConsoleUi>,
    mut events: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mut console: ResMut<Console>,
) {
    if keys.just_pressed(KeyCode::Backquote) && !ui.open {
        ui.open = true;
        ui.ignore_next = true;
        events.clear();
        return;
    }
    if !ui.open {
        events.clear();
        return;
    }
    for ev in events.read() {
        if !ev.state.is_pressed() {
            continue;
        }
        match &ev.logical_key {
            Key::Escape => ui.open = false,
            Key::Enter => {
                let line = std::mem::take(&mut ui.line);
                if !line.trim().is_empty() {
                    console.print(format!("] {line}"));
                    console.add_text(&line);
                    if ui.history.last() != Some(&line) {
                        ui.history.push(line);
                    }
                }
                ui.position = None;
            }
            Key::Backspace => {
                ui.line.pop();
            }
            Key::ArrowUp => {
                let n = ui.history.len();
                if n > 0 {
                    let p = ui.position.map_or(n - 1, |p| p.saturating_sub(1));
                    ui.position = Some(p);
                    ui.line = ui.history[p].clone();
                }
            }
            Key::ArrowDown => {
                if let Some(p) = ui.position {
                    if p + 1 < ui.history.len() {
                        ui.position = Some(p + 1);
                        ui.line = ui.history[p + 1].clone();
                    } else {
                        ui.position = None;
                        ui.line.clear();
                    }
                }
            }
            Key::Character(c) => {
                if ev.key_code == KeyCode::Backquote {
                    if ui.ignore_next {
                        ui.ignore_next = false;
                    } else {
                        ui.open = false;
                    }
                    continue;
                }
                ui.ignore_next = false;
                ui.line.push_str(c);
            }
            Key::Space => ui.line.push(' '),
            _ => {}
        }
    }
}

fn paint(ui: Res<ConsoleUi>, console: Res<Console>, schemes: Res<Schemes>, mut surface: ResMut<Surface>) {
    if !ui.open {
        return;
    }
    let scheme = &schemes.source;
    let (sw, sh) = (surface.wide, surface.tall);
    let (w, h) = (sw * 2 / 3, sh * 2 / 3);
    let (x, y) = ((sw - w) / 2, (sh - h) / 2);
    surface.set_layer(layer::CONSOLE);
    let bg = scheme.setting_color("Frame.BgColor").unwrap_or([62, 70, 55, 255]);
    let border = scheme.setting_color("Border.Bright").unwrap_or([136, 145, 128, 255]);
    let text = scheme.setting_color("Console.TextColor").unwrap_or([216, 222, 211, 255]);
    let entry_bg = scheme.setting_color("TextEntry.BgColor").unwrap_or([62, 70, 55, 255]);
    surface.fill(x, y, x + w, y + h, [bg[0], bg[1], bg[2], 255]);
    surface.outline(x, y, x + w, y + h, border);
    let font_name = if scheme.has_font("ConsoleText") { "ConsoleText" } else { "DefaultSmall" };
    let Some(spec) = scheme.font_spec(font_name, sh, false) else { return };
    let font = surface.font(spec);
    let line_h = surface.font_tall(font).max(1);
    let title_font = scheme.font_spec("UiBold", sh, false).map(|s| surface.font(s)).unwrap_or(font);
    surface.draw_text(title_font, x + 8, y + 4, "Console", text);
    let (ox, oy, ow, oh) = (x + 8, y + 28, w - 16, h - 28 - line_h - 20);
    surface.fill(ox, oy, ox + ow, oy + oh, [entry_bg[0] / 2, entry_bg[1] / 2, entry_bg[2] / 2, 255]);
    surface.push_clip(ox, oy, ox + ow, oy + oh);
    let rows = (oh / line_h).max(1) as usize;
    let start = console.log.len().saturating_sub(rows);
    for (i, l) in console.log[start..].iter().enumerate() {
        surface.draw_text(font, ox + 4, oy + i as i32 * line_h, l, text);
    }
    surface.pop_clip();
    let ey = y + h - line_h - 12;
    surface.fill(ox, ey - 2, ox + ow, ey + line_h + 2, [entry_bg[0] / 2, entry_bg[1] / 2, entry_bg[2] / 2, 255]);
    surface.outline(ox, ey - 2, ox + ow, ey + line_h + 2, border);
    let end = surface.draw_text(font, ox + 4, ey, &ui.line, text);
    surface.fill(end + 1, ey, end + 2, ey + line_h, text);
}
