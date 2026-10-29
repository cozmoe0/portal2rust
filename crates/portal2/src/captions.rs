//! Close captions and subtitles.
//!
//! Lines come from the game's caption dictionaries
//! (`resource/closecaption_<lang>.dat`, `subtitles_<lang>.dat`), keyed by the
//! sound entry name (or a scene event's caption token). They show only with
//! `closecaption 1`; `cc_subtitles 1` hides sound-effect (`<sfx>`) captions.
//! A caption appears `cc_predisplay_time` after its sound starts (sound
//! effects at once), stays for the sound's length (or `<len:n>`) plus
//! `cc_linger_time`, splits at `<delay:n>`, and `<norepeat:n>` suppresses
//! repeats. `<clr>` colours may be replaced by the speaker's colour from
//! `resource/captioning_colors.txt` when the text continues with `Name:`.
//! The panel follows `HudCloseCaption` in `scripts/hudlayout.res` and draws
//! like the game's `CHudCloseCaption`: lines are word-wrapped to the panel
//! width less 24 pixels in the `CloseCaption_*` fonts of
//! `basemodui_scheme`, items stack from the bottom inside a rounded box 24
//! pixels taller than the text, the box grows to new heights and fades over
//! `GrowTime`, and each item fades in after `ItemHiddenTime`.

use std::collections::HashMap;

use bevy::prelude::*;
use source_formats::captions::{Captions, Span, Tag, parse_tags};

use crate::hud::HudLayout;
use crate::vgui::layout::Rect;
use crate::vgui::{FontId, Schemes, Surface, VguiPaint, layer};
use portal2_core::clock::GameClock;
use portal2_core::console::Console;
use portal2_core::vfs::GameFs;

const DEFAULT_COLOR: [u8; 3] = [255, 255, 255];
/// Text inset inside the panel (pixels, not scaled).
const INSET: i32 = 12;

pub struct CaptionsPlugin;

impl Plugin for CaptionsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CaptionState>().add_systems(PostUpdate, draw_captions.in_set(VguiPaint));
        let mut console = app.world_mut().resource_mut::<Console>();
        console.register_cvar("cc_minvisibleitems", "1", 0);
        for (name, default) in
            [("closecaption", "0"), ("cc_subtitles", "0"), ("cc_lang", ""), ("cc_linger_time", "1.0"), ("cc_predisplay_time", "0.25")]
        {
            console.register_cvar(name, default, portal2_core::console::cvars::flags::ARCHIVE);
        }
    }
}

#[derive(Clone, Debug)]
struct Run {
    text: String,
    color: [u8; 3],
    bold: bool,
    italic: bool,
}

#[derive(Clone, Debug)]
struct Item {
    runs: Vec<Run>,
    start: f64,
    end: f64,
    layout: Option<Layout>,
}

/// An item's text broken into lines, as runs of one font and colour.
#[derive(Clone, Debug)]
struct Layout {
    wrap: i32,
    generation: u32,
    units: Vec<Unit>,
    height: i32,
}

#[derive(Clone, Debug)]
struct Unit {
    x: i32,
    y: i32,
    font: FontId,
    color: [u8; 3],
    text: String,
}

#[derive(Resource, Default)]
pub struct CaptionState {
    dict: Option<Captions>,
    colors: HashMap<String, [u8; 3]>,
    items: Vec<Item>,
    recent: HashMap<String, f64>,
    /// Box height and alpha: shown, aimed at, and the current transition.
    height: i32,
    target_height: i32,
    alpha: f32,
    target_alpha: f32,
    anim: (f64, f64),
}

fn load(world: &mut World) {
    if world.resource::<CaptionState>().dict.is_some() {
        return;
    }
    let Some(fs) = world.get_resource::<GameFs>().cloned() else { return };
    let lang = world.resource::<Console>().str("cc_lang").trim().to_owned();
    let lang = if lang.is_empty() { "english".to_owned() } else { lang };
    let dict = Captions::load(&fs.0, &lang);
    let mut colors = HashMap::new();
    if let Some(text) = fs.read_string("resource/captioning_colors.txt") {
        for root in source_formats::kv::parse_document(&format!("\"colors\" {{ {text} }}")).roots {
            for c in root.children() {
                let rgb = c.as_str().and_then(|v| {
                    parse_tags(v).into_iter().find_map(|s| match s {
                        Span::Tag(Tag::Color(Some(rgb))) => Some(rgb),
                        _ => None,
                    })
                });
                if let Some(rgb) = rgb {
                    colors.insert(c.key.to_ascii_lowercase(), rgb);
                }
            }
        }
    }
    info!("captions: {} entries ({lang})", dict.len());
    let mut s = world.resource_mut::<CaptionState>();
    s.dict = Some(dict);
    s.colors = colors;
}

/// Queues the caption for a sound or scene token that just started.
pub fn emit(world: &mut World, token: &str, duration: f32, from_player: bool) {
    load(world);
    let now = world.resource::<GameClock>().curtime;
    let (predisplay, linger, subtitles_only) = {
        let c = world.resource::<Console>();
        (c.f32("cc_predisplay_time"), c.f32("cc_linger_time"), c.bool("cc_subtitles"))
    };
    let mut state = world.resource_mut::<CaptionState>();
    let Some(text) = state.dict.as_ref().and_then(|d| d.lookup(token)) else { return };
    if text.starts_with("!!!") || text.starts_with("-->") {
        return;
    }
    let spans = parse_tags(&text);
    let sfx = spans.iter().any(|s| matches!(s, Span::Tag(Tag::Sfx)));
    if sfx && subtitles_only {
        return;
    }
    let key = token.to_ascii_lowercase();
    if let Some(n) = spans.iter().find_map(|s| match s {
        Span::Tag(Tag::NoRepeat(n)) => Some(*n),
        _ => None,
    }) {
        if state.recent.get(&key).is_some_and(|&t| now - t < n as f64) {
            return;
        }
        state.recent.insert(key, now);
    }
    let length = spans
        .iter()
        .find_map(|s| match s {
            Span::Tag(Tag::Length(n)) => Some(*n),
            _ => None,
        })
        .unwrap_or(duration);
    let start = now + if sfx { 0.0 } else { predisplay as f64 };
    let end = start + (length + linger).max(0.5) as f64;
    // Split at <delay>, tracking colour, bold and italic.
    let mut pieces: Vec<(f32, Vec<Run>)> = vec![(0.0, Vec::new())];
    let mut stack: Vec<[u8; 3]> = vec![DEFAULT_COLOR];
    let (mut bold, mut italic) = (false, false);
    let mut i = 0;
    while i < spans.len() {
        match &spans[i] {
            Span::Text(t) => {
                let color = *stack.last().unwrap();
                pieces.last_mut().unwrap().1.push(Run { text: t.clone(), color, bold, italic });
            }
            Span::Tag(Tag::Color(Some(c))) => {
                // "Name:" right after the colour picks the speaker's colour.
                let speaker = match spans.get(i + 1) {
                    Some(Span::Text(t)) => t.split_once(':').and_then(|(n, _)| state.colors.get(&n.trim().to_ascii_lowercase())).copied(),
                    _ => None,
                };
                stack.push(speaker.unwrap_or(*c));
            }
            Span::Tag(Tag::Color(None)) => {
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            Span::Tag(Tag::PlayerColor { player, other }) => stack.push(if from_player { *player } else { *other }),
            Span::Tag(Tag::Bold) => bold = !bold,
            Span::Tag(Tag::Italic) => italic = !italic,
            Span::Tag(Tag::LineBreak) => {
                let color = *stack.last().unwrap();
                pieces.last_mut().unwrap().1.push(Run { text: "\n".into(), color, bold, italic });
            }
            Span::Tag(Tag::Delay(d)) => pieces.push((*d, Vec::new())),
            Span::Tag(_) => {}
        }
        i += 1;
    }
    for (delay, runs) in pieces {
        if runs.iter().all(|r| r.text.trim().is_empty()) {
            continue;
        }
        state.items.push(Item { runs, start: start + delay as f64, end: end.max(start + delay as f64 + 0.5), layout: None });
    }
}

/// The game's item fade: hidden for `hidden` seconds, then in over
/// `fade_in`, and out over the last `fade_out` seconds.
fn item_alpha(item: &Item, now: f64, hidden: f32, fade_in: f32, fade_out: f32) -> f32 {
    let since = (now - item.start) as f32;
    let left = (item.end - now) as f32;
    let mut a: f32 = if since < hidden {
        0.0
    } else if fade_in > 0.0 && since < hidden + fade_in {
        (since - hidden) / fade_in
    } else {
        1.0
    };
    if fade_out > 0.0 && left < fade_out {
        a = a.min(left / fade_out);
    }
    a.clamp(0.0, 1.0)
}

/// Word wrap as the game does it: characters are added to the line while
/// they fit; a character that does not fit moves the word since the last
/// space onto a new line (the space is dropped), or starts a new line when
/// the line has no space to break at.
fn layout_item(surface: &mut Surface, fonts: &[FontId; 4], line_tall: i32, runs: &[Run], wrap: i32) -> (Vec<Unit>, i32) {
    struct Glyph {
        ch: char,
        font: FontId,
        color: [u8; 3],
        width: i32,
    }
    let mut lines: Vec<Vec<Glyph>> = vec![Vec::new()];
    let mut line_w = 0;
    // Index into the current line just after its last space.
    let mut brk: Option<usize> = None;
    for run in runs {
        let font = fonts[(run.bold as usize) * 2 + run.italic as usize];
        for ch in run.text.chars() {
            if ch == '\n' {
                lines.push(Vec::new());
                line_w = 0;
                brk = None;
                continue;
            }
            let width = surface.char_width(font, ch);
            if line_w + width > wrap {
                match brk {
                    Some(b) if b < lines.last().unwrap().len() => {
                        let line = lines.last_mut().unwrap();
                        let moved: Vec<Glyph> = line.drain(b..).collect();
                        // Drop the space the line broke at.
                        if line.last().is_some_and(|g| g.ch.is_whitespace()) {
                            line.pop();
                        }
                        line_w = moved.iter().map(|g| g.width).sum();
                        lines.push(moved);
                    }
                    _ => {
                        lines.push(Vec::new());
                        line_w = 0;
                    }
                }
                brk = None;
            }
            lines.last_mut().unwrap().push(Glyph { ch, font, color: run.color, width });
            line_w += width;
            if ch.is_whitespace() {
                brk = Some(lines.last().unwrap().len());
            }
        }
    }
    let mut units = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let mut x = 0;
        let mut cur: Option<Unit> = None;
        for g in line {
            match &mut cur {
                Some(u) if u.font == g.font && u.color == g.color => u.text.push(g.ch),
                _ => {
                    if let Some(u) = cur.take() {
                        units.push(u);
                    }
                    cur = Some(Unit { x, y: i as i32 * line_tall, font: g.font, color: g.color, text: g.ch.to_string() });
                }
            }
            x += g.width;
        }
        units.extend(cur);
    }
    (units, lines.len() as i32 * line_tall)
}

fn draw_captions(
    clock: Res<GameClock>,
    console: Res<Console>,
    schemes: Res<Schemes>,
    hud: Res<HudLayout>,
    mut surface: ResMut<Surface>,
    mut state: ResMut<CaptionState>,
) {
    let now = clock.curtime;
    state.items.retain(|i| i.end > now);
    if !console.bool("closecaption") || !crate::hud::visible(&console) {
        return;
    }
    let tall = surface.tall;
    let panel = hud.rect("HudCloseCaption", &surface, Rect::default());
    if panel.w <= 0 || panel.h <= 0 {
        return;
    }
    let grow = hud.f32("HudCloseCaption", "GrowTime", 0.25) as f64;
    let bg_alpha = hud.f32("HudCloseCaption", "BgAlpha", 128.0) as i32;
    let hidden = hud.f32("HudCloseCaption", "ItemHiddenTime", 0.2);
    let fade_in = hud.f32("HudCloseCaption", "ItemFadeInTime", 0.15);
    let fade_out = hud.f32("HudCloseCaption", "ItemFadeOutTime", 0.3);
    let top_offset = hud.f32("HudCloseCaption", "topoffset", 0.0) as i32;
    let font = |name: &str, s: &mut Surface| schemes.basemodui.font_spec(name, tall, true).map(|f| s.font(f));
    let Some(normal) = font("CloseCaption_Normal", &mut surface) else { return };
    let italic = font("CloseCaption_Italic", &mut surface).unwrap_or(normal);
    let bold = font("CloseCaption_Bold", &mut surface).unwrap_or(normal);
    let bold_italic = font("CloseCaption_BoldItalic", &mut surface).unwrap_or(bold);
    let fonts = [normal, italic, bold, bold_italic];
    let line_tall = surface.font_tall(normal).max(6);
    let wrap = panel.w - 2 * INSET;
    let avail = panel.h - top_offset - 2 * INSET;
    let generation = schemes.generation;
    let keep = console.f32("cc_minvisibleitems").max(0.0) as usize;

    // Items whose predisplay time is over, laid out, newest last.
    let mut shown: Vec<usize> = Vec::new();
    let mut total = 0;
    for i in 0..state.items.len() {
        if state.items[i].start > now {
            continue;
        }
        let stale = state.items[i].layout.as_ref().is_none_or(|l| l.wrap != wrap || l.generation != generation);
        if stale {
            let runs = state.items[i].runs.clone();
            let (units, height) = layout_item(&mut surface, &fonts, line_tall, &runs, wrap);
            state.items[i].layout = Some(Layout { wrap, generation, units, height });
        }
        shown.push(i);
        total += state.items[i].layout.as_ref().map_or(0, |l| l.height);
        // Too tall: drop the oldest, keeping `cc_minvisibleitems`.
        while total > avail && shown.len() > keep.max(1) {
            let old = shown.remove(0);
            total -= state.items[old].layout.as_ref().map_or(0, |l| l.height);
            state.items[old].end = now;
        }
    }
    let target_alpha = if shown.is_empty() { 0.0 } else { 1.0 };
    if total != state.target_height || target_alpha != state.target_alpha {
        state.target_height = total;
        state.target_alpha = target_alpha;
        state.anim = (now, now + grow);
    }
    if shown.is_empty() && state.height == state.target_height && state.alpha == state.target_alpha {
        state.anim = (0.0, 0.0);
        return;
    }
    let (a0, a1) = state.anim;
    let mut shrinking = false;
    if a1 != 0.0 && a0 != 0.0 && a1 > a0 {
        let f = (((now - a0) / (a1 - a0)) as f32).clamp(0.0, 1.0);
        let dh = (state.target_height - state.height) as f32;
        shrinking = dh < 0.0;
        state.height += (f * dh) as i32;
        state.alpha = (state.alpha + f * (state.target_alpha - state.alpha)).clamp(0.0, 1.0);
    } else {
        state.height = state.target_height;
        state.alpha = state.target_alpha;
    }

    surface.set_layer(layer::HUD);
    let box_y = (panel.h - state.height - 2 * INSET).max(0);
    let bg = [0, 0, 0, (bg_alpha.clamp(0, 255) as f32 * state.alpha) as u8];
    crate::vgui::widgets::draw_box(&mut surface, panel.x, panel.y + box_y, panel.w, panel.h - box_y, bg);
    if shown.is_empty() {
        return;
    }
    let mut y = panel.y + panel.h - if shrinking { total } else { state.height } - INSET;
    let x = panel.x + INSET;
    surface.push_clip(panel.x, panel.y + top_offset, panel.right(), panel.bottom());
    for i in shown {
        let item = &state.items[i];
        let a = item_alpha(item, now, hidden, fade_in, fade_out);
        let Some(layout) = item.layout.as_ref() else { continue };
        for u in &layout.units {
            if y + u.y + line_tall > panel.bottom() {
                break;
            }
            let color = [u.color[0], u.color[1], u.color[2], (255.0 * a) as u8];
            surface.draw_text(u.font, x + u.x + 3, y + u.y, &u.text, color);
        }
        y += layout.height;
        if y >= panel.bottom() {
            break;
        }
    }
    surface.pop_clip();
}
