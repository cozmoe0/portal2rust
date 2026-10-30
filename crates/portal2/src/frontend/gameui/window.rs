//! A BaseModUI window: the controls of its layout file, placed, painted and
//! navigated like the game's `CBaseModFrame` and its hybrid buttons.
//!
//! Dialog-style windows (`dialogstyle 1`) are sized in tiles
//! (`Dialog.TileWidth/TileHeight`; the file's `wide`/`tall` give the count,
//! a titled dialog is one row taller), pinned `Dialog.PinFromLeft` from the
//! left and `Dialog.PinFromBottom` above the bottom of the screen, and drawn
//! from the `vgui/menu_tiles` corner, edge and interior tiles.

use source_formats::kv::KeyValues;

use portal2_game::screen::Localization;
use portal2_ui::vgui::layout;
use portal2_ui::vgui::layout::{Rect, ResFile};
use portal2_ui::vgui::scheme::{Scheme, proportional};
use portal2_ui::vgui::surface::Rgba;
use portal2_ui::vgui::{FontId, Surface};

/// What a hybrid button looks like: the scheme's `<Style>.*` settings with
/// `HybridButton.*` defaults.
#[derive(Clone, Debug)]
pub struct ButtonStyle {
    pub style: i32,
    pub font: String,
    pub text_color: Rgba,
    pub focus_color: Rgba,
    pub cursor_color: Rgba,
    pub disabled_color: Rgba,
    pub text_inset_x: i32,
    pub text_inset_y: i32,
    pub cursor_height: i32,
    pub all_caps: bool,
}

impl ButtonStyle {
    pub fn from_scheme(scheme: &Scheme, name: &str) -> Self {
        let get = |key: &str| {
            scheme.setting(&format!("{name}.{key}")).or_else(|| scheme.setting(&format!("HybridButton.{key}"))).map(str::to_owned)
        };
        let color = |key: &str, default: Rgba| {
            scheme
                .setting_color(&format!("{name}.{key}"))
                .or_else(|| scheme.setting_color(&format!("HybridButton.{key}")))
                .unwrap_or(default)
        };
        let int = |key: &str| get(key).map(|v| source_formats::kv::parse_i32_lenient(&v)).unwrap_or(0);
        Self {
            style: int("Style"),
            font: get("Font").unwrap_or_else(|| "DialogMenuItem".into()),
            text_color: color("TextColor", [0, 0, 0, 255]),
            focus_color: color("FocusColor", [255, 255, 255, 255]),
            cursor_color: color("CursorColor", [0, 0, 0, 0]),
            disabled_color: color("DisabledColor", [140, 140, 140, 255]),
            text_inset_x: int("TextInsetX"),
            text_inset_y: int("TextInsetY"),
            cursor_height: int("CursorHeight"),
            all_caps: int("AllCaps") != 0,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Kind {
    Button(ButtonStyle),
    Label { font: Option<String>, color: Option<Rgba>, align: String },
    Image { image: String, color: Rgba },
    Other,
}

#[derive(Clone, Debug)]
pub struct Control {
    pub name: String,
    pub kind: Kind,
    pub settings: KeyValues,
    /// Relative to the window.
    pub rect: Rect,
    pub visible: bool,
    pub enabled: bool,
    pub zpos: i32,
    pub text: String,
    pub command: String,
    pub nav_up: Option<String>,
    pub nav_down: Option<String>,
}

impl Control {
    pub fn is_button(&self) -> bool {
        matches!(self.kind, Kind::Button(_))
    }
}

pub struct Window {
    pub name: String,
    pub file: String,
    pub controls: Vec<Control>,
    pub rect: Rect,
    pub dialog: bool,
    pub title: String,
    /// Tiles across and down (dialog style).
    pub tiles: (i32, i32),
    pub focus: Option<usize>,
    /// Built for this scheme generation (layout is redone when it changes).
    pub generation: u32,
}

fn localize(loc: &Localization, s: &str) -> String {
    loc.text(s)
}

impl Window {
    pub fn new(name: &str, file: &str, title: &str) -> Self {
        Self {
            name: name.to_owned(),
            file: file.to_owned(),
            controls: Vec::new(),
            rect: Rect::default(),
            dialog: false,
            title: title.to_owned(),
            tiles: (0, 0),
            focus: None,
            generation: u32::MAX,
        }
    }

    /// Loads (or reloads) the layout for the current screen.
    pub fn layout(&mut self, res: &ResFile, scheme: &Scheme, loc: &Localization, screen: (i32, i32), generation: u32) {
        let focus_name = self.focus.and_then(|i| self.controls.get(i)).map(|c| c.name.clone());
        let tall = screen.1;
        let full = Rect::new(0, 0, screen.0, screen.1);
        let own = res.get(&self.name).cloned().unwrap_or_else(|| KeyValues::new_block(self.name.clone(), Vec::new()));
        self.dialog = own.get_i32("dialogstyle").unwrap_or(0) != 0;
        if self.dialog {
            // The body: whole tiles pinned to the lower left; a title tab
            // sits on top of it.
            self.tiles = (own.get_i32("wide").unwrap_or(5), own.get_i32("tall").unwrap_or(4));
            let (tw, th) = super::dialog::tile_size(scheme, tall);
            let w = tw * self.tiles.0;
            let h = th * self.tiles.1;
            let x = proportional(scheme.setting_i32("Dialog.PinFromLeft").unwrap_or(75), tall);
            let y = tall - proportional(scheme.setting_i32("Dialog.PinFromBottom").unwrap_or(75), tall) - h;
            self.rect = Rect::new(x, y, w, h);
        } else {
            self.rect = layout::apply(&own, full, screen, full, true);
        }
        self.controls.clear();
        for c in &res.controls {
            if c.key.eq_ignore_ascii_case(&self.name) {
                continue;
            }
            let control_name = c.get_str("ControlName").unwrap_or("");
            let kind = match control_name.to_ascii_lowercase().as_str() {
                "basemodhybridbutton" => Kind::Button(ButtonStyle::from_scheme(scheme, c.get_str("style").unwrap_or("DefaultButton"))),
                "label" => Kind::Label {
                    font: c.get_str("font").map(str::to_owned),
                    color: c.get_str("fgcolor_override").and_then(|v| scheme.color(v)),
                    align: c.get_str("textAlignment").unwrap_or("west").to_ascii_lowercase(),
                },
                "imagepanel" => Kind::Image {
                    image: c.get_str("image").unwrap_or("").to_owned(),
                    color: c.get_str("drawcolor").and_then(|v| scheme.color(v)).unwrap_or([255, 255, 255, 255]),
                },
                _ => Kind::Other,
            };
            let mut rect = layout::apply(c, Rect::default(), screen, self.rect, true);
            if self.dialog {
                // Dialog controls are placed from the body's corner; buttons
                // without a width span the body.
                rect = rect.offset(self.rect.x, self.rect.y);
                if let Kind::Button(_) = kind
                    && rect.w == 0
                {
                    rect.w = self.rect.w;
                }
            }
            let text = c.get_str("labelText").map(|t| localize(loc, t)).unwrap_or_default();
            self.controls.push(Control {
                name: c.key.clone(),
                kind,
                settings: c.clone(),
                rect,
                visible: c.get_i32("visible").unwrap_or(1) != 0,
                enabled: c.get_i32("enabled").unwrap_or(1) != 0,
                zpos: c.get_i32("zpos").unwrap_or(0),
                text,
                command: c.get_str("command").unwrap_or("").to_owned(),
                nav_up: c.get_str("navUp").map(str::to_owned),
                nav_down: c.get_str("navDown").map(str::to_owned),
            });
        }
        self.focus = focus_name.and_then(|n| self.index(&n)).or_else(|| self.first_button());
        self.generation = generation;
    }

    pub fn index(&self, name: &str) -> Option<usize> {
        self.controls.iter().position(|c| c.name.eq_ignore_ascii_case(name))
    }

    pub fn control_mut(&mut self, name: &str) -> Option<&mut Control> {
        let i = self.index(name)?;
        self.controls.get_mut(i)
    }

    fn usable(&self, i: usize) -> bool {
        self.controls.get(i).is_some_and(|c| c.is_button() && c.visible && c.enabled)
    }

    pub fn first_button(&self) -> Option<usize> {
        let mut buttons: Vec<usize> = (0..self.controls.len()).filter(|&i| self.usable(i)).collect();
        buttons.sort_by_key(|&i| (self.controls[i].rect.y, self.controls[i].rect.x));
        buttons.first().copied()
    }

    /// Follows `navUp`/`navDown` from the focused button, skipping hidden or
    /// disabled ones; falls back to screen order.
    pub fn navigate(&mut self, down: bool) -> bool {
        let Some(start) = self.focus.or_else(|| self.first_button()) else { return false };
        let mut cur = start;
        for _ in 0..self.controls.len() {
            let next = {
                let c = &self.controls[cur];
                let target = if down { &c.nav_down } else { &c.nav_up };
                target.as_ref().and_then(|n| self.index(n))
            };
            let next = match next {
                Some(n) => n,
                None => {
                    let mut buttons: Vec<usize> = (0..self.controls.len()).filter(|&i| self.usable(i)).collect();
                    buttons.sort_by_key(|&i| (self.controls[i].rect.y, self.controls[i].rect.x));
                    let pos = buttons.iter().position(|&i| i == cur).unwrap_or(0);
                    let n = buttons.len().max(1);
                    buttons.get(if down { (pos + 1) % n } else { (pos + n - 1) % n }).copied().unwrap_or(cur)
                }
            };
            if next == start {
                break;
            }
            cur = next;
            if self.usable(cur) {
                let changed = self.focus != Some(cur);
                self.focus = Some(cur);
                return changed;
            }
        }
        false
    }

    /// The button under a screen position.
    pub fn hit(&self, x: i32, y: i32) -> Option<usize> {
        (0..self.controls.len()).filter(|&i| self.usable(i)).find(|&i| self.controls[i].rect.contains(x, y))
    }

    pub fn paint(&self, surface: &mut Surface, scheme: &Scheme, alt: bool) {
        let tall = surface.tall;
        if self.dialog {
            let tab = super::dialog::tab_tiles(surface, scheme, &self.title);
            super::dialog::paint_tiles(surface, scheme, self.rect, tab, alt);
            super::dialog::paint_title(surface, scheme, self.rect, &self.title, alt);
        }
        let mut order: Vec<usize> = (0..self.controls.len()).filter(|&i| self.controls[i].visible).collect();
        order.sort_by_key(|&i| self.controls[i].zpos);
        for i in order {
            let c = &self.controls[i];
            match &c.kind {
                Kind::Button(style) => paint_button(surface, scheme, c, style, self.focus == Some(i)),
                Kind::Label { font, color, align } => {
                    let Some(spec) = font.as_deref().and_then(|f| scheme.font_spec(f, tall, true)) else { continue };
                    let font = surface.font(spec);
                    let color = color.unwrap_or([255, 255, 255, 255]);
                    paint_label(surface, font, c.rect, &c.text, color, align);
                }
                Kind::Image { image, color, .. } => {
                    if image.is_empty() {
                        continue;
                    }
                    let name = if image.starts_with("..") { image.trim_start_matches("../").to_owned() } else { format!("vgui/{image}") };
                    if let Some(img) = surface.image(&name) {
                        surface.draw_image(&img, c.rect.x, c.rect.y, c.rect.right(), c.rect.bottom(), *color);
                    }
                }
                Kind::Other => {}
            }
        }
    }
}

pub fn paint_label(surface: &mut Surface, font: FontId, rect: Rect, text: &str, color: Rgba, align: &str) {
    let w = surface.text_width(font, text);
    let h = surface.font_tall(font);
    let x = match align {
        "north" | "center" | "south" => rect.x + (rect.w - w) / 2,
        "north-east" | "east" | "south-east" => rect.right() - w,
        _ => rect.x,
    };
    let y = match align {
        "north-west" | "north" | "north-east" => rect.y,
        "south-west" | "south" | "south-east" => rect.bottom() - h,
        _ => rect.y + (rect.h - h) / 2,
    };
    surface.draw_text(font, x, y, text, color);
}

/// A hybrid button: a `CursorHeight` bar from the top of the focused one,
/// then its label in the style's colours, `TextInsetX/Y` in from the
/// button's corner. Bitmap buttons draw their icon first.
fn paint_button(surface: &mut Surface, scheme: &Scheme, c: &Control, style: &ButtonStyle, focused: bool) {
    let tall = surface.tall;
    let Some(spec) = scheme.font_spec(&style.font, tall, true) else { return };
    let font = surface.font(spec);
    let r = c.rect;
    if focused && c.enabled {
        let h = if style.cursor_height > 0 { proportional(style.cursor_height, tall) } else { r.h };
        surface.fill(r.x, r.y, r.right(), r.y + h, style.cursor_color);
    }
    let mut text_x = r.x;
    if style.style == 10 {
        let key = if focused { "bitmap_focus" } else { "bitmap_enabled" };
        let name = c.settings.get_str(key).or_else(|| c.settings.get_str("bitmap_enabled"));
        if let Some(img) = name.and_then(|n| surface.image(n)) {
            surface.draw_image(&img, r.x, r.y, r.x + r.h, r.y + r.h, [255, 255, 255, 255]);
        }
        text_x += r.h;
    }
    let color = if !c.enabled {
        style.disabled_color
    } else if focused {
        style.focus_color
    } else {
        style.text_color
    };
    let text = if style.all_caps { c.text.to_uppercase() } else { c.text.clone() };
    let x = text_x + proportional(style.text_inset_x, tall);
    let y = r.y + proportional(style.text_inset_y, tall);
    surface.draw_text(font, x, y, &text, color);
}
