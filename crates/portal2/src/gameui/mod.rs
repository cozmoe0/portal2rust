//! The game's menus (BaseModUI): startup videos, the main menu over its
//! background movie, the single player dialogs, the pause menu and the
//! confirmation dialogs, driven by keyboard and mouse like the original.
//!
//! The main menu background is `media/menu_act<N>.bik` (or the still
//! `vgui/backgrounds/background<N>` with `-nomenuvid`), with the music
//! `music/mainmenu/portal2_background<N>.wav`; N is the act of the chapter
//! of the most recent save (falling back to `ui_lastact_played`), as the
//! game picks it. See `docs/re/gameui.md`.

pub mod console_ui;
pub mod dialog;
pub mod window;

use portal2_core::view::UiFocus;
use std::collections::HashMap;

use bevy::app::AppExit;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::screen::Localization;
use crate::vgui::layout::ResFile;
use crate::vgui::scheme::proportional;
use crate::vgui::{Schemes, Surface, VguiPaint, layer};
use crate::video::{VideoId, Videos};
use crate::world::CurrentMap;
use portal2_core::args::Args;
use portal2_core::console::Console;
use portal2_core::vfs::GameFs;
use window::Window;

/// Single-player maps by chapter, and the act of each (from the game's
/// chapter table: the act is the `sp_a<N>` of the chapter's first map).
pub const CHAPTERS: [&str; 10] = [
    "sp_a1_intro1",
    "sp_a2_laser_intro",
    "sp_a2_sphere_peek",
    "sp_a2_column_blocker",
    "sp_a2_bts3",
    "sp_a3_00",
    "sp_a3_speed_ramp",
    "sp_a4_intro",
    "sp_a4_finale1",
    "sp_a5_credits",
];

/// The chapter (1-based) a map belongs to.
pub fn chapter_of(map: &str) -> Option<usize> {
    const MAPS: [(&str, usize); 63] = [
        ("sp_a1_intro1", 1),
        ("sp_a1_intro2", 1),
        ("sp_a1_intro3", 1),
        ("sp_a1_intro4", 1),
        ("sp_a1_intro5", 1),
        ("sp_a1_intro6", 1),
        ("sp_a1_intro7", 1),
        ("sp_a1_wakeup", 1),
        ("sp_a2_intro", 1),
        ("sp_a2_laser_intro", 2),
        ("sp_a2_laser_stairs", 2),
        ("sp_a2_dual_lasers", 2),
        ("sp_a2_laser_over_goo", 2),
        ("sp_a2_catapult_intro", 2),
        ("sp_a2_trust_fling", 2),
        ("sp_a2_pit_flings", 2),
        ("sp_a2_fizzler_intro", 2),
        ("sp_a2_sphere_peek", 3),
        ("sp_a2_ricochet", 3),
        ("sp_a2_bridge_intro", 3),
        ("sp_a2_bridge_the_gap", 3),
        ("sp_a2_turret_intro", 3),
        ("sp_a2_laser_relays", 3),
        ("sp_a2_turret_blocker", 3),
        ("sp_a2_laser_vs_turret", 3),
        ("sp_a2_pull_the_rug", 3),
        ("sp_a2_column_blocker", 4),
        ("sp_a2_laser_chaining", 4),
        ("sp_a2_triple_laser", 4),
        ("sp_a2_bts1", 4),
        ("sp_a2_bts2", 4),
        ("sp_a2_bts3", 5),
        ("sp_a2_bts4", 5),
        ("sp_a2_bts5", 5),
        ("sp_a2_bts6", 5),
        ("sp_a2_core", 5),
        ("sp_a3_00", 6),
        ("sp_a3_01", 6),
        ("sp_a3_03", 6),
        ("sp_a3_jump_intro", 6),
        ("sp_a3_bomb_flings", 6),
        ("sp_a3_crazy_box", 6),
        ("sp_a3_transition01", 6),
        ("sp_a3_speed_ramp", 7),
        ("sp_a3_speed_flings", 7),
        ("sp_a3_portal_intro", 7),
        ("sp_a3_end", 7),
        ("sp_a4_intro", 8),
        ("sp_a4_tb_intro", 8),
        ("sp_a4_tb_trust_drop", 8),
        ("sp_a4_tb_wall_button", 8),
        ("sp_a4_tb_polarity", 8),
        ("sp_a4_tb_catch", 8),
        ("sp_a4_stop_the_box", 8),
        ("sp_a4_laser_catapult", 8),
        ("sp_a4_laser_platform", 8),
        ("sp_a4_speed_tb_catch", 8),
        ("sp_a4_jump_polarity", 8),
        ("sp_a4_finale1", 9),
        ("sp_a4_finale2", 9),
        ("sp_a4_finale3", 9),
        ("sp_a4_finale4", 9),
        ("sp_a5_credits", 10),
    ];
    MAPS.iter().find(|(m, _)| m.eq_ignore_ascii_case(map)).map(|(_, c)| *c)
}

/// The act shown behind the main menu for a chapter.
pub fn act_of_chapter(chapter: usize) -> i32 {
    let first = CHAPTERS[chapter.clamp(1, CHAPTERS.len()) - 1];
    first.strip_prefix("sp_a").and_then(|r| r[..1].parse().ok()).unwrap_or(1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Startup videos.
    Intro,
    /// No map: the main menu.
    Menu,
    /// In a map; menus only when paused.
    Game,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Confirm {
    Quit,
    ExitToMenu,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    MainMenu,
    SinglePlayer,
    NewGame,
    LoadGame,
    InGameMenu,
    Confirm(Confirm),
}

struct Open {
    kind: Kind,
    window: Window,
    /// New Game: the selected chapter (0-based) and the first one shown.
    chapter: usize,
    scroll: usize,
    /// Footer buttons as last painted, and the one under the mouse.
    footer: Vec<crate::vgui::layout::Rect>,
    footer_hover: Option<usize>,
}

impl Open {
    fn new(kind: Kind, window: Window) -> Self {
        Self { kind, window, chapter: 0, scroll: 0, footer: Vec::new(), footer_hover: None }
    }
}

/// Footer buttons of each dialog, as localization tokens.
fn footer_labels(kind: &Kind) -> &'static [&'static str] {
    match kind {
        Kind::MainMenu => &[],
        Kind::SinglePlayer => &["#L4D360UI_Back"],
        Kind::NewGame => &["#PORTAL2_ButtonAction_Play", "#L4D360UI_Back"],
        Kind::LoadGame => &["#PORTAL2_ButtonAction_Load", "#L4D360UI_Back"],
        Kind::InGameMenu => &["#L4D360UI_Select", "#L4D360UI_Done"],
        Kind::Confirm(Confirm::Quit) => &["#PORTAL2_ButtonAction_Quit", "#L4D360UI_Cancel"],
        Kind::Confirm(Confirm::ExitToMenu) => &["#L4D360UI_Ok", "#L4D360UI_Cancel"],
    }
}

/// Chapters 1-9 with their two-line names (`SP_PRESENCE_TEXT_CH<n>`).
fn chapter_names(loc: &Localization) -> Vec<(String, String)> {
    (1..=9)
        .map(|n| {
            let text = loc.text(&format!("#SP_PRESENCE_TEXT_CH{n}"));
            let text = text.replace("\\n", "\n");
            match text.split_once('\n') {
                Some((a, b)) => (a.trim().to_owned(), b.trim().to_owned()),
                None => (format!("Chapter {n}"), text),
            }
        })
        .collect()
}

#[derive(Resource)]
pub struct GameUi {
    pub mode: Mode,
    windows: Vec<Open>,
    intro: Vec<String>,
    intro_video: Option<VideoId>,
    background: Option<(i32, Option<VideoId>)>,
    music: Option<u64>,
    files: HashMap<String, ResFile>,
    /// Set when a window wants the world (commands run in `apply`).
    actions: Vec<Action>,
    enabled: bool,
    was_active: bool,
    console_open: bool,
    /// The loading screen's poster frames and when it went up.
    loading: Option<(Vec<String>, f64)>,
}

enum Action {
    Sound(&'static str),
    Console(String),
    Quit,
}

impl Default for GameUi {
    fn default() -> Self {
        Self {
            mode: Mode::Menu,
            windows: Vec::new(),
            intro: Vec::new(),
            intro_video: None,
            background: None,
            music: None,
            files: HashMap::new(),
            actions: Vec::new(),
            enabled: true,
            was_active: false,
            console_open: false,
            loading: None,
        }
    }
}

impl GameUi {
    /// Whether a menu (or the console) has the keyboard and mouse.
    pub fn active(&self) -> bool {
        self.console_open || (self.enabled && (self.mode != Mode::Game || !self.windows.is_empty()))
    }
}

pub struct GameUiPlugin;

impl Plugin for GameUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameUi>()
            .init_resource::<UiFocus>()
            .add_systems(Startup, start)
            .add_systems(Update, (track_mode, input, apply).chain())
            .add_systems(PostUpdate, paint.in_set(VguiPaint));
        let mut console = app.world_mut().resource_mut::<Console>();
        console.register_cvar("ui_lastact_played", "0", portal2_core::console::cvars::flags::ARCHIVE);
        console_ui::build(app);
        let mut console = app.world_mut().resource_mut::<Console>();
        console.register_command("gameui_activate", |world, _| open_pause_menu(world));
        console.register_command("gameui_hide", |world, _| close_all(world));
        console.register_command("disconnect", |world, _| crate::world::unload_map(world));
        // Development aid: opens a menu by name (singleplayer, newgame, quit).
        console.register_command("ui_open", |world, args| {
            let kind = match args.arg(1).to_ascii_lowercase().as_str() {
                "singleplayer" => Kind::SinglePlayer,
                "newgame" => Kind::NewGame,
                "loadgame" => Kind::LoadGame,
                "quit" => Kind::Confirm(Confirm::Quit),
                "pause" => Kind::InGameMenu,
                _ => return,
            };
            let mut ui = world.resource_mut::<GameUi>();
            open(&mut ui, kind);
        });
    }
}

fn start(mut ui: ResMut<GameUi>, args: Res<Args>, fs: Res<GameFs>, mut pending: ResMut<crate::world::PendingLoad>) {
    let batch = args.batch();
    // `P2R_UI=1` keeps the menus in screenshot runs (for comparing them).
    let batch = batch && std::env::var_os("P2R_UI").is_none();
    if batch {
        ui.enabled = false;
        ui.mode = Mode::Game;
        return;
    }
    pending.enabled = true;
    if !args.map.is_empty() {
        ui.mode = Mode::Game;
        return;
    }
    if !args.novid {
        if let Some(list) = fs.read_string("media/startupvids.txt") {
            ui.intro = list.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("//")).map(str::to_owned).collect();
        }
    }
    ui.mode = if ui.intro.is_empty() { Mode::Menu } else { Mode::Intro };
}

/// Follows map loads: entering a map closes the menus; losing it shows the
/// main menu again. Leaving the menus in a game captures the mouse again.
fn track_mode(
    mut ui: ResMut<GameUi>,
    map: Option<Res<CurrentMap>>,
    mut time: ResMut<Time<Virtual>>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut console: ResMut<Console>,
    console_ui: Res<console_ui::ConsoleUi>,
    mut focus: ResMut<UiFocus>,
) {
    ui.console_open = console_ui.open;
    focus.0 = ui.active();
    if ui.console_open
        && let Ok(mut c) = cursor.single_mut()
        && c.grab_mode != CursorGrabMode::None
    {
        c.grab_mode = CursorGrabMode::None;
        c.visible = true;
    }
    if !ui.enabled {
        ui.was_active = ui.active();
        return;
    }
    if let Some(map) = map.as_ref()
        && map.is_changed()
        && let Some(chapter) = chapter_of(&map.name)
    {
        let act = act_of_chapter(chapter).to_string();
        let _ = console.set("ui_lastact_played", &act);
    }
    match (ui.mode, map.is_some()) {
        (Mode::Menu, true) => {
            ui.mode = Mode::Game;
            ui.windows.clear();
        }
        (Mode::Game, false) => ui.mode = Mode::Menu,
        _ => {}
    }
    // The single-player game pauses under the pause menu.
    let paused = ui.mode == Mode::Game && !ui.windows.is_empty();
    if paused != time.is_paused() {
        if paused { time.pause() } else { time.unpause() }
    }
    let active = ui.active();
    if ui.was_active
        && !active
        && ui.mode == Mode::Game
        && let Ok(mut c) = cursor.single_mut()
    {
        c.grab_mode = CursorGrabMode::Locked;
        c.visible = false;
    }
    ui.was_active = active;
    if ui.mode == Mode::Menu && ui.windows.is_empty() {
        ui.windows.push(Open::new(Kind::MainMenu, Window::new("MainMenu", "resource/ui/basemodui/mainmenu_new.res", "")));
    }
}

fn open(ui: &mut GameUi, kind: Kind) {
    let (name, file, title) = match &kind {
        Kind::MainMenu => ("MainMenu", "mainmenu_new", ""),
        Kind::SinglePlayer => ("SinglePlayer", "singleplayer", "#L4D360UI_GameSettings_Solo"),
        Kind::NewGame => ("NewGameDialog", "newgamedialog", "#PORTAL2_NewGame"),
        Kind::LoadGame => ("NewGameDialog", "newgamedialog", "#PORTAL2_LoadGame"),
        Kind::InGameMenu => ("InGameMainMenu", "ingamemainmenu", ""),
        Kind::Confirm(_) => ("GenericConfirmation", "genericconfirmation", "#L4D360UI_MainMenu_Quit_Confirm"),
    };
    let file = format!("resource/ui/basemodui/{file}.res");
    ui.windows.push(Open::new(kind, Window::new(name, &file, title)));
}

fn open_pause_menu(world: &mut World) {
    let mut ui = world.resource_mut::<GameUi>();
    if ui.enabled && ui.mode == Mode::Game && ui.windows.is_empty() {
        open(&mut ui, Kind::InGameMenu);
    }
}

fn close_all(world: &mut World) {
    let mut ui = world.resource_mut::<GameUi>();
    if ui.mode == Mode::Game {
        ui.windows.clear();
    }
}

/// Runs a button's command.
fn command(ui: &mut GameUi, kind: &Kind, cmd: &str) {
    let lower = cmd.to_ascii_lowercase();
    ui.actions.push(Action::Sound("UI/menu_accept.wav"));
    match (kind, lower.as_str()) {
        (Kind::MainMenu, "soloplay") => open(ui, Kind::SinglePlayer),
        (Kind::MainMenu, "quitgame") => open(ui, Kind::Confirm(Confirm::Quit)),
        (Kind::SinglePlayer, "btncontinuegame") => ui.actions.push(Action::Console("load_latest".into())),
        (Kind::SinglePlayer, "opennewgamedialog") => open(ui, Kind::NewGame),
        (Kind::SinglePlayer, "openloadgamedialog") => open(ui, Kind::LoadGame),
        (Kind::InGameMenu, "returntogame") => {
            ui.windows.clear();
        }
        (Kind::InGameMenu, "loadlastsave") => {
            ui.windows.clear();
            ui.actions.push(Action::Console("reload".into()));
        }
        (Kind::InGameMenu, "opensavegamedialog") => {
            ui.windows.clear();
            ui.actions.push(Action::Console("save quick".into()));
        }
        (Kind::InGameMenu, "exittomainmenu") => open(ui, Kind::Confirm(Confirm::ExitToMenu)),
        (Kind::Confirm(c), "ok" | "yes") => match c {
            Confirm::Quit => ui.actions.push(Action::Quit),
            Confirm::ExitToMenu => {
                ui.windows.clear();
                ui.actions.push(Action::Console("disconnect".into()));
            }
        },
        (Kind::Confirm(_), _) => back(ui),
        _ => {
            ui.actions.pop();
            ui.actions.push(Action::Sound("UI/menu_invalid.wav"));
        }
    }
}

/// A footer button: the first confirms (or selects), the last goes back.
fn footer_action(ui: &mut GameUi, kind: &Kind, index: usize) {
    let last = footer_labels(kind).len().saturating_sub(1);
    match kind {
        Kind::NewGame if index == 0 => {
            let chapter = ui.windows.last().map_or(0, |w| w.chapter);
            ui.actions.push(Action::Sound("UI/menu_accept.wav"));
            ui.actions.push(Action::Console(format!("map {}", CHAPTERS[chapter.min(CHAPTERS.len() - 1)])));
        }
        Kind::LoadGame if index == 0 => {
            let i = ui.windows.last().map_or(0, |w| w.chapter);
            if let Some(save) = list_saves().get(i) {
                ui.actions.push(Action::Sound("UI/menu_accept.wav"));
                ui.actions.push(Action::Console(format!("load {}", save.name)));
            }
        }
        Kind::Confirm(_) if index == 0 => command(ui, kind, "ok"),
        Kind::InGameMenu if index == 0 => {
            let cmd = ui.windows.last().and_then(|w| w.window.focus.and_then(|i| w.window.controls.get(i))).map(|c| c.command.clone());
            if let Some(cmd) = cmd {
                command(ui, kind, &cmd);
            }
        }
        Kind::InGameMenu if index == last => {
            ui.windows.clear();
            ui.actions.push(Action::Sound("UI/menu_back.wav"));
        }
        _ => back(ui),
    }
}

fn back(ui: &mut GameUi) {
    let top_is_main = ui.windows.last().is_some_and(|w| w.kind == Kind::MainMenu);
    if top_is_main {
        return;
    }
    ui.windows.pop();
    ui.actions.push(Action::Sound("UI/menu_back.wav"));
}

fn input(
    mut ui: ResMut<GameUi>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    window: Query<&bevy::window::Window, With<PrimaryWindow>>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut videos: ResMut<Videos>,
    mut last_mouse: Local<Option<(i32, i32)>>,
    console_ui: Res<console_ui::ConsoleUi>,
) {
    if !ui.enabled || console_ui.open {
        return;
    }
    // Startup videos: any key or click skips.
    if ui.mode == Mode::Intro {
        let skip = keys.get_just_pressed().next().is_some() || mouse.get_just_pressed().next().is_some();
        let done = ui.intro_video.is_some_and(|v| videos.get(v).is_none_or(|v| v.finished));
        if skip || done {
            if let Some(v) = ui.intro_video.take() {
                videos.close(v);
            }
            if !ui.intro.is_empty() {
                ui.intro.remove(0);
            }
            if ui.intro.is_empty() {
                ui.mode = Mode::Menu;
            }
        }
        return;
    }
    if ui.mode == Mode::Game && ui.windows.is_empty() {
        if keys.just_pressed(KeyCode::Escape) {
            open(&mut ui, Kind::InGameMenu);
        }
        return;
    }
    if let Ok(mut c) = cursor.single_mut() {
        if c.grab_mode != CursorGrabMode::None || !c.visible {
            c.grab_mode = CursorGrabMode::None;
            c.visible = true;
        }
    }
    let ui = &mut *ui;
    let Some(top) = ui.windows.last_mut() else { return };
    let kind = top.kind.clone();
    if keys.just_pressed(KeyCode::Escape) {
        if kind == Kind::InGameMenu {
            ui.windows.clear();
        } else {
            back(ui);
        }
        return;
    }
    let pos = window.single().ok().and_then(|w| w.physical_cursor_position()).map(|p| (p.x as i32, p.y as i32));
    top.footer_hover = pos.and_then(|(x, y)| top.footer.iter().position(|r| r.contains(x, y)));
    if mouse.just_pressed(MouseButton::Left)
        && let Some(i) = top.footer_hover
    {
        footer_action(ui, &kind, i);
        return;
    }
    if matches!(kind, Kind::NewGame | Kind::LoadGame) {
        let count = if kind == Kind::NewGame { 9 } else { list_saves().len() };
        let top = ui.windows.last_mut().unwrap();
        if keys.just_pressed(KeyCode::ArrowDown) && top.chapter + 1 < count {
            top.chapter += 1;
            ui.actions.push(Action::Sound("UI/menu_focus.wav"));
        } else if keys.just_pressed(KeyCode::ArrowUp) && top.chapter > 0 {
            top.chapter -= 1;
            ui.actions.push(Action::Sound("UI/menu_focus.wav"));
        }
        if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter) {
            footer_action(ui, &kind, 0);
        }
        return;
    }
    if let Kind::Confirm(_) = kind {
        if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter) {
            footer_action(ui, &kind, 0);
        }
        return;
    }
    let top = ui.windows.last_mut().unwrap();
    let w = &mut top.window;
    let mut moved = false;
    if keys.just_pressed(KeyCode::ArrowDown) {
        moved |= w.navigate(true);
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        moved |= w.navigate(false);
    }
    if let Some((x, y)) = pos
        && *last_mouse != pos
    {
        *last_mouse = pos;
        if let Some(i) = w.hit(x, y)
            && w.focus != Some(i)
        {
            w.focus = Some(i);
            moved = true;
        }
    }
    let clicked = mouse.just_pressed(MouseButton::Left) && pos.is_some_and(|(x, y)| w.hit(x, y).is_some());
    let activate = keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter) || clicked;
    if moved {
        ui.actions.push(Action::Sound("UI/menu_focus.wav"));
    }
    if activate && let Some(c) = w.focus.and_then(|i| w.controls.get(i)) {
        let cmd = c.command.clone();
        command(ui, &kind, &cmd);
    }
}

fn apply(world: &mut World) {
    let actions = std::mem::take(&mut world.resource_mut::<GameUi>().actions);
    for a in actions {
        match a {
            Action::Sound(s) => {
                portal2_audio::emit(world, s, portal2_audio::Emit { soundlevel: Some(0.0), ..default() });
            }
            Action::Console(c) => portal2_core::console::queue(world, &c),
            Action::Quit => {
                world.write_message(AppExit::Success);
            }
        }
    }
    update_background(world);
}

/// Starts the intro video, and the menu background movie and music for
/// the act of the latest save.
fn update_background(world: &mut World) {
    let mode = world.resource::<GameUi>().mode;
    let fs = world.resource::<GameFs>().clone();
    if mode == Mode::Intro {
        let (need, path) = {
            let ui = world.resource::<GameUi>();
            (ui.intro_video.is_none(), ui.intro.first().cloned())
        };
        if need && let Some(path) = path {
            let id = world.resource_scope(|world, mut videos: Mut<Videos>| {
                let mut surface = world.resource_mut::<Surface>();
                videos.open(&fs, &mut surface, &path, false)
            });
            let mut ui = world.resource_mut::<GameUi>();
            match id {
                Some(id) => ui.intro_video = Some(id),
                None => {
                    ui.intro.remove(0);
                    if ui.intro.is_empty() {
                        ui.mode = Mode::Menu;
                    }
                }
            }
        }
        return;
    }
    let want = mode == Mode::Menu;
    let have = world.resource::<GameUi>().background.is_some();
    if want && !have {
        let act = menu_act(world);
        let novid = world.resource::<Args>().nomenuvid;
        let video = if novid {
            None
        } else {
            let path = format!("media/menu_act{act:02}.bik");
            world.resource_scope(|world, mut videos: Mut<Videos>| {
                let mut surface = world.resource_mut::<Surface>();
                videos.open(&fs, &mut surface, &path, true)
            })
        };
        let music = format!("#music/mainmenu/portal2_background{act:02}.wav");
        let id = portal2_audio::emit(world, &music, portal2_audio::Emit { soundlevel: Some(0.0), ..default() });
        let mut ui = world.resource_mut::<GameUi>();
        ui.background = Some((act, video));
        ui.music = id;
    } else if !want && have {
        let (bg, music) = {
            let mut ui = world.resource_mut::<GameUi>();
            (ui.background.take(), ui.music.take())
        };
        if let Some((_, Some(v))) = bg {
            world.resource_mut::<Videos>().close(v);
        }
        if let Some(m) = music {
            portal2_audio::stop_channel(world, m);
        }
    }
}

/// The act for the menu background: the chapter of the newest save, else
/// `ui_lastact_played`, clamped to 1..=5 (`-background N` overrides).
fn menu_act(world: &World) -> i32 {
    let args = world.resource::<Args>();
    if let Some(n) = args.background {
        return n.clamp(1, 5);
    }
    let from_save = crate::save::newest_save_map().and_then(|m| chapter_of(&m)).map(act_of_chapter);
    let act = from_save.unwrap_or_else(|| {
        let v = world.resource::<Console>().f32("ui_lastact_played") as i32;
        if (0..5).contains(&v) { v } else { 0 }
    });
    act.clamp(1, 5)
}

/// The loading screen poster for a map (`LoadingProgress`): the act's
/// poster when loading from the menu or entering an act, else a default
/// one; every numbered frame of it, wide-screen versions when wide.
fn loading_posters(fs: &GameFs, map: &str, in_game: bool, wide: bool) -> Vec<String> {
    let lower = map.to_ascii_lowercase();
    let mut act = lower.strip_prefix("sp_a").and_then(|r| r.get(..1)).and_then(|d| d.parse::<i32>().ok()).unwrap_or(0);
    let mut use_act = !in_game;
    if lower == "sp_a3_00" || lower == "sp_a3_01" {
        act = 2;
    } else if lower == "sp_a3_03" {
        act = 3;
        use_act = true;
    }
    if in_game && act > 0 && chapter_of(&lower).is_some_and(|c| CHAPTERS[c - 1] == lower) {
        use_act = true;
    }
    let base = if act > 0 && use_act && lower != "sp_a5_credits" {
        format!("vgui/loading_screens/loadingscreen_a{act}")
    } else {
        "vgui/loading_screens/loadingscreen_default_a".to_owned()
    };
    let suffix = if wide { "_widescreen" } else { "" };
    let mut frames = Vec::new();
    for n in 1..16 {
        let name = format!("{base}_{n}{suffix}");
        if !fs.exists(&format!("materials/{name}.vmt")) {
            break;
        }
        frames.push(name);
    }
    frames
}

fn paint_loading(ui: &mut GameUi, surface: &mut Surface, now: f64) {
    let Some((frames, start)) = &ui.loading else { return };
    let (sw, sh) = (surface.wide, surface.tall);
    surface.set_layer(layer::TOP);
    surface.fill(0, 0, sw, sh, [0, 0, 0, 255]);
    if frames.is_empty() {
        return;
    }
    // Frames advance every 5 seconds, cross-fading over a second.
    let t = (now - start).max(0.0);
    let i = (t / 5.0) as usize % frames.len();
    debug!("loading screen {} ({} frames)", frames[i], frames.len());
    if let Some(img) = surface.image(&frames[i]) {
        surface.draw_image(&img, 0, 0, sw, sh, [255, 255, 255, 255]);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint(
    mut ui: ResMut<GameUi>,
    pending: Res<crate::world::PendingLoad>,
    time: Res<Time<Real>>,
    schemes: Res<Schemes>,
    loc: Option<Res<Localization>>,
    fs: Res<GameFs>,
    videos: Res<Videos>,
    mut surface: ResMut<Surface>,
) {
    if !ui.enabled {
        return;
    }
    let now = time.elapsed_secs_f64();
    match (&pending.request, ui.loading.is_some()) {
        (Some(req), false) => {
            let wide = schemes.symbols.is_some_and(|s| s.wide);
            ui.loading = Some((loading_posters(&fs, &req.name, pending.in_game, wide), now));
        }
        (None, true) => ui.loading = None,
        _ => {}
    }
    if ui.loading.is_some() {
        paint_loading(&mut ui, &mut surface, now);
        return;
    }
    let (sw, sh) = (surface.wide, surface.tall);
    surface.set_layer(layer::GAMEUI);
    let scheme = &schemes.basemodui;
    let Some(symbols) = schemes.symbols else { return };
    match ui.mode {
        Mode::Intro => {
            surface.fill(0, 0, sw, sh, [0, 0, 0, 255]);
            if let Some(v) = ui.intro_video.and_then(|v| videos.get(v)) {
                draw_fit(&mut surface, &v.image, sw, sh);
            }
            return;
        }
        Mode::Menu => {
            surface.fill(0, 0, sw, sh, [0, 0, 0, 255]);
            match ui.background {
                Some((_, Some(v))) if videos.ready(v) => {
                    if let Some(v) = videos.get(v) {
                        let img = v.image;
                        surface.draw_image(&img, 0, 0, sw, sh, [255, 255, 255, 255]);
                    }
                }
                Some((act, _)) => {
                    let wide = if symbols.wide { "_widescreen" } else { "" };
                    if let Some(img) = surface.image(&format!("vgui/backgrounds/background{act:02}{wide}")) {
                        surface.draw_image(&img, 0, 0, sw, sh, [255, 255, 255, 255]);
                    }
                }
                None => {}
            }
            // The logo over the main menu.
            if ui.windows.len() <= 1 || ui.windows.last().is_some_and(|w| w.kind != Kind::MainMenu) {
                let s = |k: &str, d: i32| proportional(scheme.setting_i32(k).unwrap_or(d), sh);
                let (x, y, w, h) = (s("Logo.X", 75), s("Logo.Y", 35), s("Logo.Width", 240), s("Logo.Height", 60));
                if let Some(img) = surface.image("vgui/portal2logo") {
                    surface.draw_image(&img, x, y, x + w, y + h, [255, 255, 255, 255]);
                }
            }
        }
        Mode::Game => {
            if ui.windows.is_empty() {
                return;
            }
            surface.backdrop = true;
            let s = |k: &str, d: i32| proportional(scheme.setting_i32(k).unwrap_or(d), sh);
            let (x, y, w, h) = (s("Logo.X", 75), s("Logo.Y", 35), s("Logo.Width", 240), s("Logo.Height", 60));
            if let Some(img) = surface.image("vgui/portal2logo") {
                surface.draw_image(&img, x, y, x + w, y + h, [255, 255, 255, 255]);
            }
        }
    }
    let Some(loc) = loc else { return };
    let generation = schemes.generation;
    let ui = &mut *ui;
    for open in ui.windows.iter_mut() {
        if open.window.generation != generation {
            let file = open.window.file.clone();
            let res = ui.files.entry(file.clone()).or_insert_with(|| ResFile::load(&fs, &file, &symbols).unwrap_or_default());
            if open.window.title.starts_with('#') {
                open.window.title = loc.text(&open.window.title);
            }
            open.window.layout(res, scheme, &loc, (sw, sh), generation);
            hide_unused(open);
        }
    }
    // Only the top window draws; dialogs leave the logo showing.
    let Some(top) = ui.windows.last_mut() else { return };
    let alt = false;
    match top.kind.clone() {
        Kind::NewGame => paint_new_game(&mut surface, scheme, &loc, top),
        Kind::LoadGame => paint_load_game(&mut surface, scheme, &loc, top),
        Kind::Confirm(c) => paint_confirm(&mut surface, scheme, &loc, &c, top),
        _ => top.window.paint(&mut surface, scheme, alt),
    }
    let labels: Vec<String> = footer_labels(&top.kind).iter().map(|t| loc.text(t)).collect();
    if !labels.is_empty() {
        let body = top.window.rect;
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        top.footer = dialog::paint_footer(&mut surface, scheme, body, &refs, top.footer_hover);
    }
}

/// New Game: the chapter list, the chapter's picture and its name.
fn paint_new_game(surface: &mut Surface, scheme: &crate::vgui::Scheme, loc: &Localization, open: &mut Open) {
    let tall = surface.tall;
    let body = open.window.rect;
    let tab = dialog::tab_tiles(surface, scheme, &open.window.title);
    dialog::paint_tiles(surface, scheme, body, tab, false);
    dialog::paint_title(surface, scheme, body, &open.window.title, false);
    let p = |v: i32| proportional(v, tall);
    let names = chapter_names(loc);
    let style = window::ButtonStyle::from_scheme(scheme, "DialogListButton");
    let (Some(item_spec), Some(name_spec)) =
        (scheme.font_spec("NewGameChapterName", tall, true), scheme.font_spec("NewGameChapter", tall, true))
    else {
        return;
    };
    let item_font = surface.font(item_spec.clone());
    let label_font = surface.font(name_spec);
    let bold = surface.font(item_spec);
    // ChapterList: 0,25 200x150; items 25 tall with the name at 10,2.
    let (lx, ly, lw, lh) = (body.x, body.y + p(25), p(200), p(150));
    let item_h = p(25);
    let visible = (lh / item_h.max(1)).max(1) as usize;
    if open.chapter < open.scroll {
        open.scroll = open.chapter;
    }
    if open.chapter >= open.scroll + visible {
        open.scroll = open.chapter + 1 - visible;
    }
    for (row, i) in (open.scroll..names.len()).take(visible).enumerate() {
        let y = ly + row as i32 * item_h;
        let focused = i == open.chapter;
        if focused {
            surface.fill(lx, y, lx + lw, y + p(style.cursor_height.max(25)), style.cursor_color);
        }
        let color = if focused { style.focus_color } else { style.text_color };
        let rect = crate::vgui::layout::Rect::new(lx + p(10), y + p(2), p(250), p(20));
        window::paint_label(surface, item_font, rect, &names[i].1, color, "west");
    }
    // ChapterImage: 212,15 225x152.
    if let Some(img) = surface.image(&format!("vgui/chapters/chapter{}", open.chapter + 1)) {
        let (x, y) = (body.x + p(212), body.y + p(15));
        surface.draw_image(&img, x, y, x + p(225), y + p(152), [255, 255, 255, 255]);
    }
    // ChapterText: 212,170: "Chapter N - " then the name in bold.
    let (num, name) = &names[open.chapter];
    let (x, y) = (body.x + p(212), body.y + p(170));
    let color = [0, 0, 0, 255];
    let x = surface.draw_text(label_font, x, y, &format!("{num} - "), color);
    surface.draw_text(bold, x, y, name, color);
}

/// A saved game as the Load Game dialog lists it.
struct SaveEntry {
    name: String,
    map: String,
    saved_at: u64,
}

/// Our saves, newest first.
fn list_saves() -> Vec<SaveEntry> {
    let Ok(dir) = std::fs::read_dir(crate::save::save_dir()) else { return Vec::new() };
    let mut out: Vec<SaveEntry> = dir
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("sav")))
        .filter_map(|e| {
            let name = e.path().file_stem()?.to_string_lossy().into_owned();
            let file = crate::save::read_file(&name).ok()?;
            Some(SaveEntry { name, map: file.map, saved_at: file.saved_at })
        })
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.saved_at));
    out
}

/// "Tuesday, Oct 27 8:31 PM" in local time.
fn save_date(unix: u64) -> String {
    const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (_, month, day, dow, hour, minute) = local_time(unix);
    let (h12, ampm) = match hour {
        0 => (12, "AM"),
        1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        _ => (hour - 12, "PM"),
    };
    format!("{}, {} {} {}:{:02} {}", DAYS[dow as usize % 7], MONTHS[(month as usize + 11) % 12], day, h12, minute, ampm)
}

/// (year, month 1-12, day, weekday 0 = Sunday, hour, minute).
#[cfg(windows)]
fn local_time(unix: u64) -> (u32, u32, u32, u32, u32, u32) {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ticks = (unix + 11_644_473_600) * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    // SAFETY: plain conversions between stack structures.
    unsafe {
        let mut utc: SYSTEMTIME = std::mem::zeroed();
        let mut local: SYSTEMTIME = std::mem::zeroed();
        if FileTimeToSystemTime(&ft, &mut utc) == 0 || SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) == 0 {
            return utc_time(unix);
        }
        (local.wYear as u32, local.wMonth as u32, local.wDay as u32, local.wDayOfWeek as u32, local.wHour as u32, local.wMinute as u32)
    }
}

#[cfg(not(windows))]
fn local_time(unix: u64) -> (u32, u32, u32, u32, u32, u32) {
    utc_time(unix)
}

fn utc_time(unix: u64) -> (u32, u32, u32, u32, u32, u32) {
    let days = (unix / 86400) as i64;
    let secs = unix % 86400;
    // Civil from days (proleptic Gregorian).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    let dow = (days + 4).rem_euclid(7);
    (y as u32, m as u32, d as u32, dow as u32, (secs / 3600) as u32, (secs % 3600 / 60) as u32)
}

/// Load Game: saves by date, the chapter picture of the selected one with
/// its kind of save, and the chapter name.
fn paint_load_game(surface: &mut Surface, scheme: &crate::vgui::Scheme, loc: &Localization, open: &mut Open) {
    let tall = surface.tall;
    let body = open.window.rect;
    let tab = dialog::tab_tiles(surface, scheme, &open.window.title);
    dialog::paint_tiles(surface, scheme, body, tab, false);
    dialog::paint_title(surface, scheme, body, &open.window.title, false);
    let p = |v: i32| proportional(v, tall);
    let saves = list_saves();
    let style = window::ButtonStyle::from_scheme(scheme, "DialogListButton");
    let (Some(item_spec), Some(name_spec)) =
        (scheme.font_spec("NewGameChapterName", tall, true), scheme.font_spec("NewGameChapter", tall, true))
    else {
        return;
    };
    let item_font = surface.font(item_spec.clone());
    let label_font = surface.font(name_spec);
    let (lx, ly, lw, lh) = (body.x, body.y + p(25), p(200), p(150));
    let item_h = p(25);
    let visible = (lh / item_h.max(1)).max(1) as usize;
    open.chapter = open.chapter.min(saves.len().saturating_sub(1));
    if open.chapter < open.scroll {
        open.scroll = open.chapter;
    }
    if open.chapter >= open.scroll + visible {
        open.scroll = open.chapter + 1 - visible;
    }
    for (row, i) in (open.scroll..saves.len()).take(visible).enumerate() {
        let y = ly + row as i32 * item_h;
        let focused = i == open.chapter;
        if focused {
            surface.fill(lx, y, lx + lw, y + p(style.cursor_height.max(25)), style.cursor_color);
        }
        let color = if focused { style.focus_color } else { style.text_color };
        let rect = crate::vgui::layout::Rect::new(lx + p(10), y + p(2), p(250), p(20));
        window::paint_label(surface, item_font, rect, &save_date(saves[i].saved_at), color, "west");
    }
    let Some(save) = saves.get(open.chapter) else { return };
    let chapter = chapter_of(&save.map).unwrap_or(1);
    let (ix, iy) = (body.x + p(212), body.y + p(15));
    if let Some(img) = surface.image(&format!("vgui/chapters/chapter{chapter}")) {
        surface.draw_image(&img, ix, iy, ix + p(225), iy + p(152), [255, 255, 255, 255]);
    }
    let kind = if save.name.starts_with("autosave") {
        Some(loc.text("#PORTAL2_AutoSave"))
    } else if save.name.starts_with("quick") {
        Some(loc.text("#GameUI_QuickSave"))
    } else {
        None
    };
    if let (Some(kind), Some(spec)) = (kind, scheme.font_spec("NewGameChapterName", tall, true)) {
        let f = surface.font(spec);
        let w = surface.text_width(f, &kind);
        surface.draw_text(f, ix + p(225) - w - p(4), iy + p(2), &kind, [255, 255, 255, 255]);
    }
    let names = chapter_names(loc);
    let (num, name) = &names[(chapter - 1).min(names.len() - 1)];
    let (x, y) = (body.x + p(212), body.y + p(170));
    let x = surface.draw_text(label_font, x, y, &format!("{num} - "), [0, 0, 0, 255]);
    surface.draw_text(item_font, x, y, name, [0, 0, 0, 255]);
}

/// A confirmation: the info icon and the message in a tiled body.
fn paint_confirm(surface: &mut Surface, scheme: &crate::vgui::Scheme, loc: &Localization, c: &Confirm, open: &mut Open) {
    let tall = surface.tall;
    let p = |v: i32| proportional(v, tall);
    let (tw, th) = dialog::tile_size(scheme, tall);
    let message = match c {
        Confirm::Quit => loc.text("#L4D360UI_MainMenu_Quit_ConfirmMsg"),
        Confirm::ExitToMenu => loc.text("#PORTAL2_MsgBx_ConfirmExitGame"),
    };
    let Some(spec) = scheme.font_spec(scheme.setting("ConfirmationDialog.TextFont").unwrap_or("ConfirmationText"), tall, true) else {
        return;
    };
    let font = surface.font(spec);
    let text_w = surface.text_width(font, &message);
    let icon = p(32);
    let cols = ((p(9) + icon + p(18) + text_w + p(30)) + tw - 1) / tw;
    let rows = 2;
    let x = p(scheme.setting_i32("Dialog.PinFromLeft").unwrap_or(75));
    let bottom = tall - p(scheme.setting_i32("Dialog.PinFromBottom").unwrap_or(75));
    let body = crate::vgui::layout::Rect::new(x, bottom - rows * th, cols * tw, rows * th);
    open.window.rect = body;
    let tab = dialog::tab_tiles(surface, scheme, &open.window.title);
    dialog::paint_tiles(surface, scheme, body, tab, false);
    dialog::paint_title(surface, scheme, body, &open.window.title, false);
    let iy = body.y + (body.h - icon) / 2;
    if let Some(img) = surface.image("vgui/hud/icon_info") {
        surface.draw_image(&img, body.x + p(9), iy, body.x + p(9) + icon, iy + icon, [255, 255, 255, 255]);
    }
    let ty = body.y + (body.h - surface.font_tall(font)) / 2;
    surface.draw_text(font, body.x + p(9) + icon + p(15), ty, &message, [129, 129, 129, 255]);
}

/// Buttons the single-player game does not offer.
fn hide_unused(open: &mut Open) {
    let hidden: &[&str] = match open.kind {
        Kind::MainMenu => &[],
        Kind::SinglePlayer | Kind::NewGame | Kind::LoadGame => &[],
        Kind::InGameMenu => &[
            "BtnRestartLevel",
            "BtnSkipToNextLevel",
            "BtnLeaderboards",
            "BtnRateMap",
            "BtnReturnToQueue",
            "BtnSwitchToPuzzleMakerView",
            "BtnReturnToChamberCreator",
            "BtnRestartLevelPuzzle",
            "BtnRebuildPuzzle",
            "BtnExitPuzzleMaker",
        ],
        Kind::Confirm(_) => &[],
    };
    for name in hidden {
        if let Some(c) = open.window.control_mut(name) {
            c.visible = false;
        }
    }
    if open.window.focus.is_some_and(|i| !open.window.controls[i].visible) {
        open.window.focus = open.window.first_button();
    }
}

/// Draws a video frame as large as fits the screen, centred.
fn draw_fit(surface: &mut Surface, img: &crate::vgui::ImageRef, sw: i32, sh: i32) {
    let (iw, ih) = (img.width.max(1) as f32, img.height.max(1) as f32);
    let scale = (sw as f32 / iw).min(sh as f32 / ih);
    let (w, h) = ((iw * scale) as i32, (ih * scale) as i32);
    let (x, y) = ((sw - w) / 2, (sh - h) / 2);
    surface.draw_image(img, x, y, x + w, y + h, [255, 255, 255, 255]);
}
