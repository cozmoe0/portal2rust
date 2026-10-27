//! Screen effects driven by map entities: fades (`env_fade`) and on-screen
//! messages (`game_text`), plus the game's localized strings.
//!
//! Text uses a stand-in font until the game's own fonts are loaded by the
//! UI milestone; timing, colours and placement follow the entities.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::clock::GameClock;
use crate::entities::io::{InputCall, InputHandlers, fire_output};
use crate::entities::{MapEntity, SpawnHandlers};
use crate::vfs::GameFs;

pub struct ScreenPlugin;

impl Plugin for ScreenPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScreenFade>()
            .init_resource::<Messages>()
            .add_systems(Startup, (load_localization, spawn_overlay))
            .add_systems(Update, (draw_fade, draw_messages));
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
    for file in ["resource/portal2_english.txt", "resource/basemodui_english.txt", "resource/gameui_english.txt"] {
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

#[derive(Component)]
struct FadeOverlay;

fn spawn_overlay(mut commands: Commands) {
    commands.spawn((
        Name::new("screen fade"),
        FadeOverlay,
        Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
        BackgroundColor(Color::NONE),
        GlobalZIndex(100),
        Pickable::IGNORE,
    ));
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

fn draw_fade(fade: Res<ScreenFade>, clock: Res<GameClock>, mut q: Query<&mut BackgroundColor, With<FadeOverlay>>) {
    let a = fade.alpha(clock.curtime);
    let c = fade.color;
    for mut bg in &mut q {
        let color = if fade.modulate {
            // Modulation darkens toward the colour; approximate with alpha.
            Color::srgba(c.x, c.y, c.z, a * 0.5)
        } else {
            Color::srgba(c.x, c.y, c.z, a)
        };
        if bg.0 != color {
            bg.0 = color;
        }
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
    fn alpha(&self, t: f64) -> f32 {
        let since = (t - self.start) as f32;
        if since < 0.0 {
            return 0.0;
        }
        let fade_in = if self.effect == 2 { self.fade_in * self.text.chars().count() as f32 } else { self.fade_in };
        if since < fade_in {
            return if fade_in > 0.0 { since / fade_in } else { 1.0 };
        }
        let after = since - fade_in - self.hold;
        if after < 0.0 {
            return 1.0;
        }
        if self.fade_out > 0.0 { (1.0 - after / self.fade_out).max(0.0) } else { 0.0 }
    }

    fn done(&self, t: f64) -> bool {
        let fade_in = if self.effect == 2 { self.fade_in * self.text.chars().count() as f32 } else { self.fade_in };
        (t - self.start) as f32 > fade_in + self.hold + self.fade_out
    }
}

/// Active messages by channel.
#[derive(Resource, Default)]
pub struct Messages {
    pub channels: HashMap<i32, (Message, Option<Entity>)>,
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
    let old = world.resource_mut::<Messages>().channels.remove(&channel);
    if let Some((_, Some(node))) = old {
        world.entity_mut(node).despawn();
    }
    world.resource_mut::<Messages>().channels.insert(channel, (msg, None));
}

fn draw_messages(
    mut commands: Commands,
    clock: Res<GameClock>,
    console: Res<crate::console::Console>,
    mut messages: ResMut<Messages>,
    mut texts: Query<(&mut Node, &mut TextColor, &mut Text)>,
) {
    let now = clock.curtime;
    let hidden = !console.bool("cl_drawhud");
    let mut finished = Vec::new();
    for (channel, (msg, node)) in messages.channels.iter_mut() {
        if msg.done(now) {
            finished.push(*channel);
            continue;
        }
        let a = if hidden { 0.0 } else { msg.alpha(now) };
        let shown: String = if msg.effect == 2 && msg.fade_in > 0.0 {
            let n = ((now - msg.start) as f32 / msg.fade_in).floor().max(0.0) as usize;
            msg.text.chars().take(n).collect()
        } else {
            msg.text.clone()
        };
        let c = msg.color;
        let color = Color::srgba(c.x, c.y, c.z, c.w * a);
        // -1 centres on that axis: a full-width line with centred text.
        let left = if msg.x < 0.0 { Val::Px(0.0) } else { Val::Percent(msg.x * 100.0) };
        let width = if msg.x < 0.0 { Val::Percent(100.0) } else { Val::Auto };
        let top = if msg.y < 0.0 { Val::Percent(45.0) } else { Val::Percent(msg.y * 100.0) };
        match node {
            Some(n) => {
                if let Ok((mut layout, mut tc, mut text)) = texts.get_mut(*n) {
                    layout.left = left;
                    layout.top = top;
                    layout.width = width;
                    tc.0 = color;
                    if text.0 != shown {
                        text.0 = shown;
                    }
                }
            }
            None => {
                let id = commands
                    .spawn((
                        Name::new(format!("game_text channel {channel}")),
                        Text::new(shown),
                        TextFont { font_size: bevy::text::FontSize::Px(28.0), ..default() },
                        TextColor(color),
                        TextLayout::justify(if msg.x < 0.0 { Justify::Center } else { Justify::Left }),
                        Node { position_type: PositionType::Absolute, left, top, width, ..default() },
                        GlobalZIndex(90),
                    ))
                    .id();
                *node = Some(id);
            }
        }
    }
    for c in finished {
        if let Some((_, Some(n))) = messages.channels.remove(&c) {
            commands.entity(n).despawn();
        }
    }
}

impl crate::save::Restore for ScreenFade {}
