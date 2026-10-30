//! Screen effects driven by map entities: fades (`env_fade`) and on-screen
//! messages (`game_text`), plus the game's localized strings.
//!
//! Both are drawn with the interface painter: the fade in the view layer,
//! messages in the HUD with the scheme fonts of their channel.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::entities::SpawnHandlers;
use crate::entities::io::{InputCall, InputHandlers, fire_output};
use portal2_core::clock::GameClock;
use portal2_core::entity::MapEntity;
use portal2_core::vfs::GameFs;
use portal2_ui::vgui::{FontId, Schemes, Surface, VguiPaint, layer};

pub struct ScreenPlugin;

impl Plugin for ScreenPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScreenFade>()
            .init_resource::<Messages>()
            .add_systems(Startup, load_localization)
            .add_systems(PreUpdate, clear_fade_on_map_load.after(portal2_core::sets::LoadMapSet).before(portal2_core::sets::EntitySpawnSet))
            .add_systems(PostUpdate, (draw_fade, draw_messages).in_set(VguiPaint));
        let world = app.world_mut();
        {
            let mut s = world.resource_mut::<SpawnHandlers>();
            s.register("env_fade", |_, _| {});
            s.register("game_text", |_, _| {});
        }
        let mut h = world.resource_mut::<InputHandlers>();
        h.register("env_fade", fade_input);
        h.register("game_text", text_input);
    }
}

// --------------------------------------------------------------- localization

/// Tokens from the installed `resource/*_english.txt` files.
#[derive(Resource, Default)]
pub struct Localization(pub HashMap<String, String>);

impl Localization {
    /// `#token` strings resolve through the tables; others pass through.
    pub fn text(&self, s: &str) -> String {
        match s.strip_prefix('#') {
            Some(t) => self.0.get(&t.to_ascii_lowercase()).cloned().unwrap_or_else(|| s.to_owned()),
            None => s.to_owned(),
        }
    }
}

fn load_localization(mut commands: Commands, fs: Res<GameFs>) {
    let mut tokens = HashMap::new();
    for file in [
        "resource/valve_english.txt",
        "resource/vgui_english.txt",
        "resource/platform_english.txt",
        "resource/gameui_english.txt",
        "resource/portal2_english.txt",
        "resource/portal2dlc2_english.txt",
        "resource/basemodui_english.txt",
        "resource/basemodui_tu_english.txt",
    ] {
        let Some(text) = fs.read_string(file) else { continue };
        let Some(root) = source_formats::kv::parse(&text) else { continue };
        let Some(t) = root.get("tokens") else { continue };
        for kv in t.children() {
            if let Some(v) = kv.as_str() {
                tokens.insert(kv.key.to_ascii_lowercase(), v.to_owned());
            }
        }
    }
    info!("localization: {} tokens", tokens.len());
    commands.insert_resource(Localization(tokens));
}

// ------------------------------------------------------------------ env_fade

/// env_fade spawnflags.
const SF_FADE_IN: u32 = 1;
const SF_MODULATE: u32 = 2;
const SF_STAY_OUT: u32 = 8;

#[derive(Resource, Default, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct ScreenFade {
    pub color: Vec4,
    pub start: f64,
    pub duration: f32,
    pub hold: f32,
    /// From the colour to clear (else clear to the colour).
    pub fade_in: bool,
    pub stay: bool,
    pub modulate: bool,
    pub active: bool,
}

impl ScreenFade {
    /// Overlay opacity at time `t`.
    pub fn alpha(&self, t: f64) -> f32 {
        if !self.active {
            return 0.0;
        }
        let since = (t - self.start) as f32;
        let ramp = if self.duration > 0.0 { (since / self.duration).clamp(0.0, 1.0) } else { 1.0 };
        let a = self.color.w;
        if self.fade_in {
            return a * (1.0 - ramp);
        }
        if since <= self.duration + self.hold || self.stay { a * ramp } else { 0.0 }
    }
}

/// A new map starts unfaded. The fade is timed on the map's clock, which
/// starts again at zero: one left over from the last map (its exit fade,
/// which stays black) would wait for that time to come round and then
/// black the screen out for good.
fn clear_fade_on_map_load(mut loaded: MessageReader<portal2_world::MapLoaded>, mut fade: ResMut<ScreenFade>) {
    if loaded.read().last().is_some() {
        *fade = ScreenFade::default();
    }
}

/// After a restore: a fade timed after the saved clock came over from the
/// previous map (saves made before maps cleared it); it is dropped.
pub fn drop_stale_fade(world: &mut World) {
    let now = world.resource::<GameClock>().curtime;
    let mut fade = world.resource_mut::<ScreenFade>();
    if fade.active && fade.start > now {
        *fade = ScreenFade::default();
    }
}

fn fade_input(world: &mut World, call: &InputCall) -> bool {
    let reverse = match call.input.as_str() {
        "fade" => false,
        "fadereverse" => true,
        _ => return false,
    };
    let Some(me) = world.get::<MapEntity>(call.target).cloned() else { return true };
    let rgb = me.get_vec3("rendercolor").unwrap_or(Vec3::ZERO) / 255.0;
    let alpha = me.get_f32("renderamt").unwrap_or(255.0) / 255.0;
    let now = world.resource::<GameClock>().curtime;
    let fade_in = me.has_flag(SF_FADE_IN) != reverse;
    *world.resource_mut::<ScreenFade>() = ScreenFade {
        color: rgb.extend(alpha),
        start: now,
        duration: me.get_f32("duration").unwrap_or(2.0),
        hold: me.get_f32("holdtime").unwrap_or(0.0),
        fade_in,
        stay: me.has_flag(SF_STAY_OUT),
        modulate: me.has_flag(SF_MODULATE),
        active: true,
    };
    fire_output(world, call.target, "OnBeginFade", call.activator, None);
    true
}

/// The fade is part of the 3D view, under the HUD: a full-screen colour,
/// or with `modulate` a multiply of the scene towards the colour.
fn draw_fade(fade: Res<ScreenFade>, clock: Res<GameClock>, mut surface: ResMut<Surface>) {
    let a = fade.alpha(clock.curtime);
    if a <= 0.0 {
        return;
    }
    let c = fade.color;
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
    surface.set_layer(layer::VIEW);
    let (w, h) = (surface.wide, surface.tall);
    if fade.modulate {
        let m = |v: f32| byte(1.0 + (v - 1.0) * a);
        surface.modulate(0, 0, w, h, [m(c.x), m(c.y), m(c.z), 255]);
    } else {
        surface.fill(0, 0, w, h, [byte(c.x), byte(c.y), byte(c.z), byte(a)]);
    }
}

// ----------------------------------------------------------------- game_text

/// A message on one of the text channels.
#[derive(Clone, Debug, Default)]
pub struct Message {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub color: Vec4,
    pub color2: Vec4,
    pub effect: i32,
    pub fade_in: f32,
    pub fade_out: f32,
    pub hold: f32,
    pub fx_time: f32,
    pub start: f64,
}

impl Message {
    fn done(&self, t: f64) -> bool {
        let fade_in = if self.effect == 2 { self.fade_in * self.text.chars().count() as f32 } else { self.fade_in };
        (t - self.start) as f32 > fade_in + self.hold + self.fade_out
    }
}

/// Active messages by channel.
#[derive(Resource, Default)]
pub struct Messages {
    pub channels: HashMap<i32, Message>,
}

fn color_kv(me: &MapEntity, key: &str, default: Vec3) -> Vec4 {
    let parts: Vec<f32> = me.get(key).unwrap_or("").split_whitespace().filter_map(|p| p.parse().ok()).collect();
    let rgb = if parts.len() >= 3 { Vec3::new(parts[0], parts[1], parts[2]) / 255.0 } else { default };
    rgb.extend(parts.get(3).map(|a| a / 255.0).unwrap_or(1.0))
}

fn text_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let p = call.param.trim().to_owned();
    match call.input.as_str() {
        "settext" => set_kv(world, e, "message", &p),
        "setposx" => set_kv(world, e, "x", &p),
        "setposy" => set_kv(world, e, "y", &p),
        "settextcolor" => set_kv(world, e, "color", &p),
        "settextcolor2" => set_kv(world, e, "color2", &p),
        "display" => display(world, e),
        _ => return false,
    }
    true
}

fn set_kv(world: &mut World, e: Entity, key: &str, value: &str) {
    if let Some(mut m) = world.get_mut::<MapEntity>(e) {
        m.set(key, value);
    }
}

fn display(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let raw = me.get("message").unwrap_or("").replace("\\n", "\n");
    let text = world.get_resource::<Localization>().map(|l| l.text(&raw)).unwrap_or(raw);
    let msg = Message {
        text,
        x: me.get_f32("x").unwrap_or(-1.0),
        y: me.get_f32("y").unwrap_or(-1.0),
        color: color_kv(&me, "color", Vec3::splat(1.0)),
        color2: color_kv(&me, "color2", Vec3::splat(1.0)),
        effect: me.get_i32("effect").unwrap_or(0),
        fade_in: me.get_f32("fadein").unwrap_or(1.5),
        fade_out: me.get_f32("fadeout").unwrap_or(0.5),
        hold: me.get_f32("holdtime").unwrap_or(1.2),
        fx_time: me.get_f32("fxtime").unwrap_or(0.25),
        start: world.resource::<GameClock>().curtime,
    };
    let channel = me.get_i32("channel").unwrap_or(1);
    world.resource_mut::<Messages>().channels.insert(channel, msg);
}

/// One frame of a text message, following the game's `CHudMessage`: the
/// text is laid out line by line in the channel's font, each line placed by
/// `x`/`y` (-1 centres; negative values count from the right / bottom), and
/// every character gets its colour from the message effect: 0/1 fade the
/// whole message in and out, 2 types the characters out one every `fadein`
/// seconds, each flashing from `color2` to `color` over `fxtime`.
fn draw_messages(
    clock: Res<GameClock>,
    console: Res<portal2_core::console::Console>,
    schemes: Res<Schemes>,
    mut surface: ResMut<Surface>,
    mut messages: ResMut<Messages>,
) {
    let now = clock.curtime;
    messages.channels.retain(|_, m| !m.done(now));
    if !console.bool("cl_drawhud") {
        return;
    }
    surface.set_layer(layer::HUD);
    let tall = surface.tall;
    let mut channels: Vec<_> = messages.channels.iter().map(|(c, m)| (*c, m.clone())).collect();
    channels.sort_by_key(|(c, _)| *c);
    for (channel, msg) in channels {
        let font_name = match channel {
            2 => "InGameChapterTitle",
            3 => "InGameChapterSubtitle",
            _ => "InGameChapterTitle",
        };
        let Some(spec) = schemes.basemodui.font_spec(font_name, tall, true) else { continue };
        let font = surface.font(spec);
        draw_message(&mut surface, font, &msg, (now - msg.start) as f32);
    }
}

fn draw_message(surface: &mut Surface, font: FontId, msg: &Message, time: f32) {
    let text: Vec<char> = msg.text.chars().filter(|c| *c != '\r').collect();
    let lines: Vec<&[char]> = text.split(|c| *c == '\n').collect();
    let line_tall = surface.font_tall(font);
    let widths: Vec<i32> = lines.iter().map(|l| l.iter().map(|c| surface.char_width(font, *c)).sum()).collect();
    let max_width = widths.iter().copied().max().unwrap_or(0);
    let total_tall = line_tall * lines.len() as i32;
    let (sw, sh) = (surface.wide, surface.tall);
    let mut y = if msg.y == -1.0 {
        (sh - total_tall) / 2
    } else if msg.y < 0.0 {
        ((1.0 + msg.y) * sh as f32) as i32 - total_tall
    } else {
        (msg.y * sh as f32) as i32
    };
    if y + total_tall > sh {
        y = sh - total_tall;
    }
    let y = y.max(0);
    let length = text.len() as f32;
    // Whole-message blend towards "off" (255).
    let (fade_time, fade_blend) = match msg.effect {
        2 => {
            let fade_time = msg.fade_in * length + msg.hold;
            let blend = if time > fade_time && msg.fade_out > 0.0 { (time - fade_time) / msg.fade_out * 255.0 } else { 0.0 };
            (fade_time, blend as i32)
        }
        _ => {
            let fade_time = msg.fade_in + msg.hold;
            let blend = if time < msg.fade_in {
                ((msg.fade_in - time) * (1.0 / msg.fade_in) * 255.0) as i32
            } else if time > fade_time {
                if msg.fade_out > 0.0 { ((time - fade_time) / msg.fade_out * 255.0) as i32 } else { 255 }
            } else {
                0
            };
            (fade_time, blend)
        }
    };
    let alpha = (255 - fade_blend.min(255)).max(0) as u8;
    let src = msg.color.truncate() * 255.0;
    let fx = msg.color2.truncate() * 255.0;
    let mut char_time = 0.0f32;
    let mut line_y = y;
    for (line, width) in lines.iter().zip(&widths) {
        let mut x = if msg.x == -1.0 {
            (sw - width) / 2
        } else if msg.x >= 0.0 {
            (sw as f32 * msg.x) as i32
        } else {
            (sw as f64 * (msg.x + 1.0) as f64 - max_width as f64) as i32
        };
        if x + width > sw {
            x = sw - width;
        }
        let mut x = x.max(0);
        for &ch in line.iter() {
            let (mut s, mut d, mut blend) = (src, Vec3::ZERO, 0i32);
            if msg.effect == 2 {
                char_time += msg.fade_in;
                if char_time > time {
                    s = Vec3::ZERO;
                } else {
                    let delta = time - char_time;
                    if time <= fade_time {
                        if delta <= msg.fx_time {
                            d = fx;
                            blend = (255.0 - (1.0 / msg.fx_time * delta * 255.0 + 0.5)) as i32;
                        }
                    } else {
                        blend = fade_blend;
                    }
                }
            } else {
                blend = fade_blend;
            }
            let blend = blend.clamp(0, 255);
            let mix = |a: f32, b: f32| ((blend * b as i32 + a as i32 * (255 - blend)) >> 8).clamp(0, 255) as u8;
            let color = [mix(s.x, d.x), mix(s.y, d.y), mix(s.z, d.z), alpha];
            x += surface.draw_char(font, x, line_y, ch, color);
        }
        line_y += line_tall;
    }
}

impl portal2_core::save::Restore for ScreenFade {}
