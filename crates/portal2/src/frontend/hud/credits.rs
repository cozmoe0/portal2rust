//! The ending credits (`env_portal_credits` `RollPortalOutroCredits`), the
//! game's CHudPortalCredits element, from `scripts/credits.txt`
//! (docs/re/credits.md). Over the looping `media/sp_credits_bg.bik` the
//! song lyrics type out at the top left and the names type out one by one
//! at the bottom right, each new name pushing the earlier ones up; a `_`
//! cursor blinks at the end of the line being typed. Each lyric line types
//! over its own `[seconds]`; the names share `scrolltime` in proportion to
//! their length, from `scrollcreditsstart` on. Four seconds after the last
//! name the ending movie plays, then the game disconnects.

use bevy::prelude::*;
use source_formats::kv::KeyValues;

use portal2_core::console::Console;
use portal2_core::vfs::GameFs;
use portal2_game::entities::io::{InputCall, InputHandlers};
use portal2_ui::vgui::surface::Rgba;
use portal2_ui::vgui::{FontId, Schemes, Surface, VguiPaint, layer};
use portal2_ui::video::{VideoId, Videos};

const FONT: &str = "CreditsOutroText";
const BACKGROUND: &str = "media/sp_credits_bg.bik";
const ENDING: &str = "media/sp_ending_callback.bik";
/// Margins are this fraction of the screen.
const MARGIN: f32 = 0.075;
/// Names never rise above this fraction of the (letterboxed) height.
const TOP_PADDING: f32 = 0.2;
/// Lyrics sit this much lower on screens at least 720 tall.
const LYRIC_DROP: i32 = 45;
const ENDING_DELAY: f64 = 4.0;

pub fn build(app: &mut App) {
    app.init_resource::<Credits>().add_systems(PostUpdate, paint.in_set(VguiPaint));
    app.world_mut().resource_mut::<InputHandlers>().register("env_portal_credits", credits_input);
}

#[derive(Clone, Copy, Debug)]
struct Params {
    scroll_time: f32,
    separation: f32,
    blink_time: f32,
    names_start: f32,
    lyrics: Rgba,
    names: Rgba,
}

/// One lyric piece.
#[derive(Clone, Debug)]
struct Lyric {
    text: String,
    /// Text before it on the same line (`*` continuation).
    prefix: String,
    row: i32,
    start: f32,
    end: f32,
    /// `&`: everything before it goes away once it starts.
    clear: bool,
}

#[derive(Clone, Debug)]
struct Name {
    text: String,
    start: f32,
    end: f32,
}

/// The credits once rolling.
struct Run {
    params: Params,
    lyrics: Vec<Lyric>,
    names: Vec<Name>,
    started: Option<f64>,
    first_lyric: usize,
    background: Option<VideoId>,
    ending: Option<VideoId>,
    disconnected: bool,
}

#[derive(Resource, Default)]
pub struct Credits {
    run: Option<Run>,
}

fn credits_input(world: &mut World, call: &InputCall) -> bool {
    if call.input != "rollportaloutrocredits" {
        return false;
    }
    let fs = world.resource::<GameFs>().clone();
    match load(&fs) {
        Some(run) => world.resource_mut::<Credits>().run = Some(run),
        None => warn!("credits: scripts/credits.txt missing or unreadable"),
    }
    true
}

fn color(kv: &KeyValues, key: &str, default: Rgba) -> Rgba {
    let Some(s) = kv.get(key).and_then(|v| v.as_str()) else { return default };
    let v: Vec<u8> = s.split_whitespace().filter_map(|p| p.parse::<f32>().ok()).map(|f| f.clamp(0.0, 255.0) as u8).collect();
    match v.as_slice() {
        [r, g, b, a, ..] => [*r, *g, *b, *a],
        [r, g, b] => [*r, *g, *b, 255],
        _ => default,
    }
}

fn load(fs: &GameFs) -> Option<Run> {
    let text = String::from_utf8_lossy(&fs.read("scripts/credits.txt")?).into_owned();
    let root = source_formats::kv::parse(&text)?;
    let p = root.get("CreditsParams");
    let f = |key: &str, default: f32| {
        p.and_then(|p| p.get(key)).and_then(|v| v.as_str()).map(source_formats::kv::parse_f32_lenient).unwrap_or(default)
    };
    let params = Params {
        scroll_time: f("scrolltime", 57.0),
        separation: f("separation", 5.0),
        blink_time: f("cursorblinktime", 0.3),
        names_start: f("scrollcreditsstart", 6.0),
        lyrics: p.map(|p| color(p, "color_lyrics", [255; 4])).unwrap_or([255; 4]),
        names: p.map(|p| color(p, "color_credits", [255; 4])).unwrap_or([255; 4]),
    };
    let lyrics = root.get("OutroSongLyrics").map(|l| parse_lyrics(l.children())).unwrap_or_default();
    let mut names: Vec<Name> = root
        .get("OutroCreditsNames")
        .map(|n| n.children().iter().map(|c| Name { text: c.key.clone(), start: 0.0, end: 0.0 }).collect())
        .unwrap_or_default();
    // The names share the scroll time by length, one after another.
    let total: usize = names.iter().map(|n| n.text.chars().count()).sum::<usize>().max(1);
    let per_char = params.scroll_time / total as f32;
    let mut t = params.names_start;
    for n in &mut names {
        n.start = t;
        t += n.text.chars().count() as f32 * per_char;
        n.end = t;
    }
    Some(Run { params, lyrics, names, started: None, first_lyric: 0, background: None, ending: None, disconnected: false })
}

/// Lyric lines: `[seconds]text`; `*text` continues on the same line with
/// the next piece; `^` is an empty line; a lone space is a pause; `&`
/// clears the screen.
fn parse_lyrics(entries: &[KeyValues]) -> Vec<Lyric> {
    let mut out = Vec::with_capacity(entries.len());
    let (mut row, mut time) = (0, 0.0f32);
    let mut line = String::new();
    let mut continuing = false;
    for e in entries {
        let mut text = e.key.as_str();
        let mut duration = 0.0;
        if let Some(open) = text.find('[') {
            text = &text[open + 1..];
            duration = source_formats::kv::parse_f32_lenient(text);
        }
        if let Some(close) = text.find(']') {
            text = &text[close + 1..];
        }
        if let Some(i) = text.find(">>>") {
            text = &text[i + 3..];
        }
        let clear = text == "&";
        if clear {
            row = 0;
            text = " ";
        }
        let prefix = if continuing { std::mem::take(&mut line) } else { String::new() };
        continuing = false;
        let mut piece = text;
        if let Some(i) = piece.find('*') {
            piece = &piece[i + 1..];
            continuing = true;
        }
        let this_row = row;
        if piece != " " && !continuing {
            row += 1;
        }
        let shown = if piece == "^" { " " } else { piece };
        if continuing {
            line = prefix.clone() + shown;
        }
        out.push(Lyric { text: shown.to_owned(), prefix, row: this_row, start: time, end: time + duration, clear });
        time += duration;
    }
    out
}

/// The typed part of a line `progress` of the way through, `chars` long:
/// all but the last character so far, then the cursor when it is on.
fn typed(text: &str, progress: f32, cursor: bool, pad: bool) -> String {
    let len = text.chars().count();
    let chars = ((len as f32 * progress) as usize + 1).clamp(1, len.max(1));
    let mut s: String = text.chars().take(chars - 1).collect();
    s.push(if cursor { '_' } else { ' ' });
    if pad {
        while s.chars().count() < len {
            s.push(' ');
        }
    }
    s
}

fn paint(
    mut credits: ResMut<Credits>,
    clock: Res<portal2_core::clock::GameClock>,
    console: Res<Console>,
    schemes: Res<Schemes>,
    fs: Res<GameFs>,
    mut videos: ResMut<Videos>,
    mut surface: ResMut<Surface>,
    map: Option<Res<portal2_world::CurrentMap>>,
    (mut font, mut commands): (Local<Option<(i32, FontId)>>, Commands),
) {
    if map.is_none() {
        credits.run = None;
    }
    let Some(run) = credits.run.as_mut() else { return };
    if !portal2_ui::hud::visible(&console) {
        return;
    }
    let now = clock.curtime;
    let started = *run.started.get_or_insert(now);
    let t = (now - started) as f32;
    let (w, h) = (surface.wide, surface.tall);
    surface.set_layer(layer::HUD);

    if run.ending.is_none() && run.background.is_none() {
        run.background = videos.open(&fs, &mut surface, BACKGROUND, true);
    }
    // The ending movie, then back to the menu.
    if let Some(id) = run.ending {
        if let Some(v) = videos.get(id) {
            let img = v.image;
            let finished = v.finished;
            surface.fill(0, 0, w, h, [0, 0, 0, 255]);
            surface.draw_image(&img, 0, 0, w, h, [255; 4]);
            if finished && !run.disconnected {
                run.disconnected = true;
                commands.queue(|world: &mut World| portal2_core::console::queue_trusted(world, "disconnect"));
            }
        } else if !run.disconnected {
            run.disconnected = true;
            commands.queue(|world: &mut World| portal2_core::console::queue_trusted(world, "disconnect"));
        }
        return;
    }
    if let Some(v) = run.background.and_then(|id| videos.get(id)) {
        let img = v.image;
        surface.draw_image(&img, 0, 0, w, h, [255; 4]);
    }

    let font = match *font {
        Some((tall, f)) if tall == h => f,
        _ => {
            let Some(spec) = schemes.basemodui.font_spec(FONT, h, true) else { return };
            let f = surface.font(spec);
            *font = Some((h, f));
            f
        }
    };
    let line_step = surface.font_tall(font) + run.params.separation as i32;
    let blink = ((now / run.params.blink_time.max(0.01) as f64) as i64) % 2 == 0;

    // Margins, kept inside a 16:9 area.
    let (mut margin_x, mut margin_y) = ((w as f32 * MARGIN) as i32, (h as f32 * MARGIN) as i32);
    let aspect = w as f32 / h as f32;
    let mut bars = 0;
    if aspect > 16.0 / 9.0 {
        margin_x += (w - (h as f32 * 16.0 / 9.0) as i32) / 2;
    } else if aspect < 16.0 / 9.0 {
        bars = h - (w as f32 * 9.0 / 16.0) as i32;
        margin_y += bars / 2;
    }
    let top = bars / 2 + ((h - bars) as f32 * TOP_PADDING) as i32;

    // Lyrics.
    let drop = if h >= 720 { LYRIC_DROP } else { 0 };
    for (i, l) in run.lyrics.iter().enumerate() {
        if l.start > t {
            break;
        }
        if l.clear && i > run.first_lyric {
            run.first_lyric = i;
        }
    }
    for l in run.lyrics.iter().skip(run.first_lyric) {
        if l.start > t {
            break;
        }
        let current = t < l.end;
        let text = if current { typed(&l.text, (t - l.start) / (l.end - l.start).max(1e-3), blink, false) } else { l.text.clone() };
        let x = margin_x + surface.text_width(font, &l.prefix);
        let y = l.row * line_step + margin_y + drop;
        if y < -line_step || y > h + margin_y {
            continue;
        }
        surface.draw_text(font, x, y, &text, run.params.lyrics);
    }

    // Names: the one being typed sits at the bottom, earlier ones above.
    let current = run.names.iter().rposition(|n| n.start <= t);
    if let Some(c) = current {
        for (j, n) in run.names.iter().enumerate().take(c + 1) {
            let y = h - margin_y - line_step * (c - j + 1) as i32;
            if y < top || y > h - margin_y {
                continue;
            }
            let text =
                if j == c && t < n.end { typed(&n.text, (t - n.start) / (n.end - n.start).max(1e-3), blink, true) } else { n.text.clone() };
            let x = w - margin_x - surface.text_width(font, &text);
            surface.draw_text(font, x, y, &text, run.params.names);
        }
        if c + 1 == run.names.len() && (t - run.names[c].end) as f64 > ENDING_DELAY {
            if let Some(id) = run.background.take() {
                videos.close(id);
            }
            run.ending = videos.open(&fs, &mut surface, ENDING, false);
            if run.ending.is_none() {
                run.disconnected = true;
                commands.queue(|world: &mut World| portal2_core::console::queue_trusted(world, "disconnect"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(keys: &[&str]) -> Vec<KeyValues> {
        keys.iter()
            .map(|k| KeyValues { key: (*k).to_owned(), value: source_formats::kv::KvValue::Str("CreditsOutroText".into()) })
            .collect()
    }

    #[test]
    fn lyric_lines_continue_break_and_clear() {
        let l =
            parse_lyrics(&entries(&["[2.0]Forms", "[1.0]^", "[1.5]*Under the", "[1.0] circumstances", "[0.5] ", "[0.3]&", "[1.0]Again"]));
        assert_eq!(l[0].row, 0);
        assert_eq!(l[1].row, 1);
        // `*` keeps the next piece on its line, after it.
        assert_eq!((l[2].row, l[3].row), (2, 2));
        assert_eq!(l[3].prefix, "Under the");
        // A pause does not move down; `&` starts over at the top.
        assert_eq!(l[4].row, 3);
        assert!(l[5].clear);
        assert_eq!(l[6].row, 0);
        assert!((l[6].start - 6.3).abs() < 1e-4);
    }

    #[test]
    fn typing_shows_the_cursor_after_the_typed_part() {
        assert_eq!(typed("Hello", 0.0, true, false), "_");
        assert_eq!(typed("Hello", 0.5, true, false), "He_");
        assert_eq!(typed("Hello", 0.5, false, true), "He   ");
    }
}
