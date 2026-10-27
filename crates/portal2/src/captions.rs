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
//! The panel follows `HudCloseCaption` in `scripts/hudlayout.res` (500×136
//! virtual units centred at y 276, background alpha 128) and draws with the
//! scheme's `UniversLTStd-Cn` font (`vgui/fonts/universc.vfont`).

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::text::{FontSize, FontSource, FontStyle, FontWeight};
use bevy::window::PrimaryWindow;
use source_formats::captions::{Captions, Span, Tag, parse_tags};

use crate::clock::GameClock;
use crate::console::Console;
use crate::vfs::GameFs;

/// `HudCloseCaption` in `scripts/hudlayout.res`, 640×480 virtual units.
const PANEL_Y: f32 = 276.0;
const PANEL_WIDE: f32 = 500.0;
const PANEL_TALL: f32 = 136.0;
const BG_ALPHA: f32 = 128.0 / 255.0;
const FADE_IN: f32 = 0.15;
const FADE_OUT: f32 = 0.3;
/// `CloseCaption_Normal` font height.
const FONT_TALL: f32 = 20.0;
const DEFAULT_COLOR: [u8; 3] = [255, 255, 255];

pub struct CaptionsPlugin;

impl Plugin for CaptionsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CaptionState>().add_systems(Update, draw_captions);
        let mut console = app.world_mut().resource_mut::<Console>();
        for (name, default) in [
            ("closecaption", "0"),
            ("cc_subtitles", "0"),
            ("cc_lang", ""),
            ("cc_linger_time", "1.0"),
            ("cc_predisplay_time", "0.25"),
        ] {
            console.register_cvar(name, default, crate::console::cvars::flags::ARCHIVE);
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
}

#[derive(Resource, Default)]
pub struct CaptionState {
    dict: Option<Captions>,
    colors: HashMap<String, [u8; 3]>,
    items: Vec<Item>,
    recent: HashMap<String, f64>,
    fonts: Option<(Handle<Font>, Handle<Font>)>,
    panel: Option<Entity>,
    /// Text of the panel as last built.
    built: String,
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
                let rgb = c.as_str().and_then(|v| parse_tags(v).into_iter().find_map(|s| match s {
                    Span::Tag(Tag::Color(Some(rgb))) => Some(rgb),
                    _ => None,
                }));
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
        state.items.push(Item { runs, start: start + delay as f64, end: end.max(start + delay as f64 + 0.5) });
    }
}

fn draw_captions(world: &mut World) {
    let enabled = world.resource::<Console>().bool("closecaption");
    let now = world.resource::<GameClock>().curtime;
    let Some((width, height)) =
        world.query_filtered::<&Window, With<PrimaryWindow>>().iter(world).next().map(|w| (w.width(), w.height()))
    else {
        return;
    };
    world.resource_mut::<CaptionState>().items.retain(|i| i.end > now);
    let visible: Vec<(Item, f32)> = world
        .resource::<CaptionState>()
        .items
        .iter()
        .filter(|i| i.start <= now)
        .map(|i| {
            let a = (((now - i.start) as f32) / FADE_IN).min(((i.end - now) as f32) / FADE_OUT).clamp(0.0, 1.0);
            (i.clone(), a)
        })
        .collect();
    let show = enabled && !visible.is_empty();
    if world.resource::<CaptionState>().fonts.is_none()
        && let Some(fs) = world.get_resource::<GameFs>().cloned()
    {
        let mut font = |path: &str| {
            fs.read(path).and_then(|d| source_formats::vfont::decode(&d)).map(|f| world.resource_mut::<Assets<Font>>().add(Font::from_bytes(f)))
        };
        let normal = font("vgui/fonts/universc.vfont").unwrap_or_default();
        let bold = font("vgui/fonts/univercb.vfont").unwrap_or_else(|| normal.clone());
        world.resource_mut::<CaptionState>().fonts = Some((normal, bold));
    }
    let scale = height / 480.0;
    let signature = if show { visible.iter().map(|(i, _)| i.runs.iter().map(|r| r.text.as_str()).collect::<String>()).collect::<Vec<_>>().join("|") } else { String::new() };
    let (panel, fonts, rebuild) = {
        let s = world.resource::<CaptionState>();
        (s.panel, s.fonts.clone().unwrap_or_default(), s.built != signature)
    };
    if rebuild {
        if let Some(p) = panel {
            world.entity_mut(p).despawn();
        }
        world.resource_mut::<CaptionState>().panel = None;
        world.resource_mut::<CaptionState>().built = signature.clone();
        if !show {
            return;
        }
        let pad = 4.0 * scale;
        let panel = world
            .spawn((
                Name::new("close captions"),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(width * 0.5 - PANEL_WIDE * 0.5 * scale),
                    bottom: Val::Px(height - (PANEL_Y + PANEL_TALL) * scale),
                    width: Val::Px(PANEL_WIDE * scale),
                    max_height: Val::Px(PANEL_TALL * scale),
                    padding: UiRect::all(Val::Px(pad)),
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::FlexEnd,
                    overflow: Overflow::clip(),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, BG_ALPHA)),
                GlobalZIndex(80),
            ))
            .id();
        let size = FONT_TALL * scale;
        for (item, _) in &visible {
            let mut first = true;
            let mut line = Entity::PLACEHOLDER;
            for r in &item.runs {
                let font = TextFont {
                    font: FontSource::Handle(if r.bold { fonts.1.clone() } else { fonts.0.clone() }),
                    font_size: FontSize::Px(size),
                    weight: if r.bold { FontWeight(900) } else { FontWeight(500) },
                    style: if r.italic { FontStyle::Italic } else { FontStyle::Normal },
                    ..default()
                };
                let color = TextColor(Color::srgb_u8(r.color[0], r.color[1], r.color[2]));
                if first {
                    line = world.spawn((Text::new(r.text.clone()), font, color, ChildOf(panel))).id();
                    first = false;
                } else {
                    world.spawn((TextSpan::new(r.text.clone()), font, color, ChildOf(line)));
                }
            }
        }
        world.resource_mut::<CaptionState>().panel = Some(panel);
    }
    // Fades: the panel background and text follow the newest item.
    if let Some(p) = world.resource::<CaptionState>().panel {
        let a = visible.iter().map(|(_, a)| *a).fold(0.0f32, f32::max);
        if let Some(mut bg) = world.get_mut::<BackgroundColor>(p) {
            bg.0 = Color::srgba(0.0, 0.0, 0.0, BG_ALPHA * a);
        }
    }
}
