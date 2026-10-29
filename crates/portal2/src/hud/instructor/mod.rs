//! The game instructor (`C_GameInstructor`): lessons from
//! `scripts/instructor_lessons.txt` and `scripts/mod_lessons.txt` open,
//! close and succeed on game events, and the lessons being taught are drawn
//! by the locator (`locator.rs`): a static hint in the lower middle of the
//! screen when the lesson is about the player, otherwise an icon on its
//! target. Lessons stop showing once their display or success limit is
//! reached; the counts persist next to the saves. Facts in
//! docs/re/instructor.md.

pub mod lessons;
pub mod locator;

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;

use lessons::{LessonDef, LessonVars, Outcome, ScriptEnv};
use portal2_core::events::GameEvent;

use portal2_core::console::{CommandArgs, Console};
use portal2_core::entity::{EntityRegistry, MapEntity};
use portal2_core::vfs::GameFs;

pub fn build(app: &mut App) {
    app.add_message::<GameEvent>()
        .init_resource::<GameInstructor>()
        .add_systems(Update, (load_lessons, handle_events, update_lessons).chain());
    let mut console = app.world_mut().resource_mut::<Console>();
    // The installed game runs with the first three changed from their
    // defaults (1.0, 2 and 2.25; seen with the console in the game).
    for (name, value) in [
        ("locator_icon_min_size_non_ss", "1.5"),
        ("locator_icon_max_size_non_ss", "1.75"),
        ("locator_lerp_rest", "0"),
        ("locator_fade_time", "0.3"),
        ("locator_lerp_speed", "5.0"),
        ("locator_lerp_time", "1.75"),
        ("locator_pulse_time", "1.0"),
        ("locator_target_offset_x", "0"),
        ("locator_target_offset_y", "0"),
    ] {
        console.register_cvar(name, value, 0);
    }
    console.register_cvar("gameinstructor_verbose", "0", portal2_core::console::cvars::flags::CHEAT);
    console.register_command("gameinstructor_reset_counts", reset_counts);
    console.register_command("gameinstructor_dump_open_lessons", dump_open_lessons);
    locator::build(app);
}

/// An open lesson ("opportunity").
pub struct OpenLesson {
    pub def: Arc<LessonDef>,
    pub vars: LessonVars,
    /// When the lesson started being taught (shown), if it is.
    pub started: Option<f64>,
    /// When it last stopped being taught, for the fade out.
    pub stopped: Option<f64>,
    /// Ids the locator keys its per-target state on.
    pub id: u64,
}

#[derive(Resource, Default)]
pub struct GameInstructor {
    defs: Vec<Arc<LessonDef>>,
    pub open: Vec<OpenLesson>,
    /// Lessons that closed while shown, fading out.
    pub closing: Vec<(OpenLesson, f64)>,
    /// Times each lesson was shown and succeeded.
    counts: HashMap<String, (i32, i32)>,
    counts_loaded: bool,
    priority: i32,
    next_id: u64,
    loaded: bool,
}

impl GameInstructor {
    fn learned(&self, def: &LessonDef) -> bool {
        let (shown, succeeded) = self.counts.get(&def.name).copied().unwrap_or((0, 0));
        (def.display_limit > 0 && shown >= def.display_limit) || (def.success_limit > 0 && succeeded >= def.success_limit)
    }

    fn bump(&mut self, name: &str, shown: i32, succeeded: i32) {
        let c = self.counts.entry(name.to_owned()).or_default();
        c.0 += shown;
        c.1 += succeeded;
        save_counts(&self.counts);
    }
}

fn counts_path() -> std::path::PathBuf {
    crate::save::save_dir().join("game_instructor_counts.txt")
}

fn load_counts() -> HashMap<String, (i32, i32)> {
    let Ok(text) = std::fs::read_to_string(counts_path()) else { return HashMap::new() };
    text.lines()
        .filter_map(|l| {
            let mut p = l.rsplitn(3, '\t');
            let succeeded = p.next()?.parse().ok()?;
            let shown = p.next()?.parse().ok()?;
            Some((p.next()?.to_owned(), (shown, succeeded)))
        })
        .collect()
}

fn save_counts(counts: &HashMap<String, (i32, i32)>) {
    let mut lines: Vec<String> = counts.iter().map(|(k, (d, s))| format!("{k}\t{d}\t{s}")).collect();
    lines.sort();
    let path = counts_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, lines.join("\n")) {
        warn!("game instructor counts: {e}");
    }
}

fn reset_counts(world: &mut World, _: &CommandArgs) {
    let mut gi = world.resource_mut::<GameInstructor>();
    gi.counts.clear();
    save_counts(&gi.counts);
}

fn dump_open_lessons(world: &mut World, _: &CommandArgs) {
    let lines: Vec<String> = world
        .resource::<GameInstructor>()
        .open
        .iter()
        .map(|l| format!("{} ({}){}", l.def.name, l.vars.string1, if l.started.is_some() { " [shown]" } else { "" }))
        .collect();
    let mut console = world.resource_mut::<Console>();
    console.print("GAME INSTRUCTOR: Open lessons...");
    for l in lines {
        console.print(&format!("  {l}"));
    }
}

fn verbose(console: &Console, text: &str) {
    if console.i32("gameinstructor_verbose") > 0 {
        info!("GAME INSTRUCTOR: {text}");
    }
}

fn load_lessons(mut gi: ResMut<GameInstructor>, fs: Res<GameFs>, console: Res<Console>) {
    if !gi.counts_loaded {
        gi.counts = load_counts();
        gi.counts_loaded = true;
    }
    if gi.loaded {
        return;
    }
    gi.loaded = true;
    let convar = |name: &str| console.cvar(name).map(|c| c.f32());
    for file in ["scripts/instructor_lessons.txt", "scripts/mod_lessons.txt"] {
        if let Some(text) = fs.read_string(file) {
            let defs = lessons::parse_lessons(&text, &convar);
            gi.defs.extend(defs);
        }
    }
}

/// Opens, succeeds and closes lessons for this frame's events.
fn handle_events(
    mut events: MessageReader<GameEvent>,
    mut gi: ResMut<GameInstructor>,
    console: Res<Console>,
    registry: Res<EntityRegistry>,
    entities: Query<&MapEntity>,
    clock: Res<portal2_core::clock::GameClock>,
) {
    let enabled = console.bool("gameinstructor_enable");
    let classname = |e: Entity| {
        if Some(e) == registry.player {
            return Some("player".to_owned());
        }
        entities.get(e).ok().map(|m| m.classname.clone())
    };
    for event in events.read() {
        if !enabled {
            continue;
        }
        let env = ScriptEnv { event: Some(event), player: registry.player, classname: &classname };
        // Open lessons first see whether this succeeds or closes them.
        let mut i = 0;
        while i < gi.open.len() {
            let def = gi.open[i].def.clone();
            let succeeded = def.success.iter().filter(|h| h.event == event.name).any(|h| {
                let mut vars = gi.open[i].vars.clone();
                let ok = lessons::run(h, &mut vars, &env) == Outcome::Passed;
                if ok {
                    gi.open[i].vars = vars;
                }
                ok
            });
            if succeeded {
                verbose(&console, &format!("Lesson \"{}\" succeeded.", def.name));
                gi.bump(&def.name, 0, 1);
            }
            let closes = succeeded
                || def.close.iter().filter(|h| h.event == event.name).any(|h| {
                    let mut vars = gi.open[i].vars.clone();
                    lessons::run(h, &mut vars, &env) == Outcome::Passed
                });
            if closes {
                verbose(&console, &format!("Opportunity \"{}\" closed for reason: Close event elements completed.", def.name));
                close(&mut gi, i, clock.curtime);
            } else {
                i += 1;
            }
        }
        // Then lessons this event opens.
        let defs = gi.defs.clone();
        for def in defs.iter() {
            for h in def.open.iter().filter(|h| h.event == event.name) {
                let mut vars = def.defaults.clone();
                if lessons::run(h, &mut vars, &env) != Outcome::Passed {
                    continue;
                }
                open(&mut gi, def, vars, &env, &console);
            }
        }
    }
}

fn close(gi: &mut GameInstructor, index: usize, now: f64) {
    let lesson = gi.open.remove(index);
    if lesson.started.is_some() {
        gi.closing.push((lesson, now));
    }
}

/// `C_GameInstructor::OpenOpportunity`.
fn open(gi: &mut GameInstructor, def: &Arc<LessonDef>, vars: LessonVars, env: &ScriptEnv, console: &Console) {
    if def.once_learned_never_open && gi.learned(def) {
        verbose(console, &format!("Opportunity \"{}\" NOT opened (learned).", def.name));
        return;
    }
    match def.instance_type {
        // One open lesson of a kind: the open one takes the new values.
        1 => {
            if let Some(l) = gi.open.iter_mut().find(|l| Arc::ptr_eq(&l.def, def)) {
                l.vars = vars;
                return;
            }
        }
        // Replace the newest open one with the same name and replace key
        // once there are as many as allowed.
        2 => {
            let mut count = 0;
            let mut candidate = None;
            for (i, l) in gi.open.iter().enumerate().rev() {
                if Arc::ptr_eq(&l.def, def) && l.vars.replace_key.eq_ignore_ascii_case(&vars.replace_key) {
                    count += 1;
                    if !def.replace_only_when_stopped || l.started.is_none() {
                        candidate = Some(i);
                    }
                    if count >= def.fixed_instances_max {
                        break;
                    }
                }
            }
            if count >= def.fixed_instances_max {
                let Some(i) = candidate else {
                    verbose(console, &format!("Opportunity \"{}\" NOT opened (there is too many started lessons of this type).", def.name));
                    return;
                };
                verbose(console, &format!("Opportunity \"{}\" replacing open lesson of same type.", def.name));
                // The replacement carries on where the old one was shown.
                let old = gi.open.remove(i);
                let id = gi.next_id;
                gi.next_id += 1;
                let mut lesson = OpenLesson { def: def.clone(), vars, started: old.started, stopped: None, id };
                if old.started.is_some() {
                    lesson.id = old.id;
                }
                run_onopen(&mut lesson, env);
                verbose(console, &format!("Opportunity \"{}\" opened.", def.name));
                gi.open.push(lesson);
                return;
            }
        }
        _ => {}
    }
    let id = gi.next_id;
    gi.next_id += 1;
    let mut lesson = OpenLesson { def: def.clone(), vars, started: None, stopped: None, id };
    run_onopen(&mut lesson, env);
    verbose(console, &format!("Opportunity \"{}\" opened.", def.name));
    gi.open.push(lesson);
}

fn run_onopen(lesson: &mut OpenLesson, env: &ScriptEnv) {
    for h in &lesson.def.onopen {
        let mut vars = lesson.vars.clone();
        if lessons::run(h, &mut vars, env) == Outcome::Passed {
            lesson.vars = vars;
        }
    }
}

/// `C_GameInstructor::Update`: decides which open lessons are taught now,
/// by priority, runs their update scripts and times them out.
#[allow(clippy::too_many_arguments)]
fn update_lessons(
    mut gi: ResMut<GameInstructor>,
    console: Res<Console>,
    clock: Res<portal2_core::clock::GameClock>,
    registry: Res<EntityRegistry>,
    entities: Query<&MapEntity>,
    transforms: Query<&GlobalTransform>,
    map: Option<Res<crate::world::CurrentMap>>,
) {
    let now = clock.curtime;
    let fade = console.f32("locator_fade_time").max(0.0) as f64;
    gi.closing.retain(|(_, at)| now - at < fade);
    if map.is_none() || !console.bool("gameinstructor_enable") {
        return;
    }
    // Lessons about entities that no longer exist end.
    gi.open.retain(|l| l.vars.icon_target.is_none_or(|t| transforms.contains(t)));
    let classname = |e: Entity| {
        if Some(e) == registry.player {
            return Some("player".to_owned());
        }
        entities.get(e).ok().map(|m| m.classname.clone())
    };
    let env = ScriptEnv { event: None, player: registry.player, classname: &classname };
    let player_pos = registry.player.and_then(|p| transforms.get(p).ok()).map(|t| t.translation());
    let mut priority = 0;
    let mut shown_roots: Vec<*const LessonDef> = Vec::new();
    let mut i = gi.open.len();
    while i > 0 {
        i -= 1;
        // Update scripts may close the lesson.
        let def = gi.open[i].def.clone();
        let mut closed = false;
        for h in &def.update {
            let mut vars = gi.open[i].vars.clone();
            match lessons::run(h, &mut vars, &env) {
                Outcome::Close => closed = true,
                Outcome::Passed => gi.open[i].vars = vars,
                Outcome::Failed => {}
            }
        }
        if closed {
            close(&mut gi, i, now);
            continue;
        }
        let l = &gi.open[i];
        if let Some(start) = l.started
            && l.vars.timeout > 0.0
            && now - start >= l.vars.timeout as f64
        {
            verbose(&console, &format!("Opportunity \"{}\" closed for reason: Timed out.", def.name));
            close(&mut gi, i, now);
            continue;
        }
        let in_range = l.vars.range <= 0.0
            || match (player_pos, l.vars.icon_target.and_then(|t| transforms.get(t).ok())) {
                (Some(p), Some(t)) => p.distance(t.translation()) <= l.vars.range,
                _ => true,
            };
        let one_shown = def.instance_type == 3 && shown_roots.contains(&Arc::as_ptr(&def));
        let wants = l.vars.visible
            && l.vars.icon_target.is_some()
            && in_range
            && !one_shown
            && !gi.learned(&def)
            && (def.priority >= gi.priority || def.priority == 0);
        let l = &mut gi.open[i];
        match (wants, l.started) {
            (true, None) => {
                l.started = Some(now);
                l.stopped = None;
                verbose(&console, &format!("Started lesson \"{}\".", def.name));
                verbose(&console, &format!("Lesson \"{}\" marked as displayed.", def.name));
                let name = def.name.clone();
                gi.bump(&name, 1, 0);
            }
            (false, Some(_)) => {
                l.started = None;
                l.stopped = Some(now);
                verbose(&console, &format!("Stopped lesson \"{}\".", def.name));
            }
            _ => {}
        }
        if wants {
            shown_roots.push(Arc::as_ptr(&def));
            priority = priority.max(def.priority);
        }
    }
    gi.priority = priority;
}
