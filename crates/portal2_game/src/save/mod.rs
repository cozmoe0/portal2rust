//! Saved games: `save`, `load`, `quicksave`, `quickload`, autosaves and
//! `reload`.
//!
//! A save holds what is needed to carry on: the map, the game clock, every
//! map entity (keyvalues, outputs, placement, parent, visibility and the state
//! of its components), the I/O event queue, global states, the script VM and
//! the player. Loading reloads the map as a "load game" and, instead of
//! spawning the map's entities, spawns the saved ones and puts their state
//! back; their scripts are not run again, the VM comes back as it was.
//!
//! Files are gzip-compressed CBOR in our own folder (never the game's `SAVE`
//! folder), named like the game's: `quick.sav`, `autosave.sav`,
//! `autosavedangerous.sav`, with one older copy of quick saves and autosaves
//! kept (`quick01.sav`, `autosave01.sav`).

mod autosave;
mod registry;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::entities::EntityDefinitions;
use crate::entities::io::{Connection, EventQueue, Outputs};
use crate::player::Player;
use portal2_core::clock::GameClock;
use portal2_core::console::{CommandArgs, Console};
use portal2_core::entity::{EntityRegistry, MapEntity};
use portal2_core::save::ids::{self, PLAYER_ID, SaveId};
use portal2_core::save::{SaveRegistry, decode, encode};
use portal2_render::models::StudioAnimator;
use portal2_world::{CurrentMap, LoadMap, MapLoadType};

const VERSION: u32 = 1;

pub struct SavePlugin;

impl Plugin for SavePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ids::NextSaveId>()
            .init_resource::<SaveState>()
            .add_systems(PreUpdate, finish_restore.after(portal2_core::sets::TransitionSet))
            .add_systems(Update, registry::place_saved_portals.after(crate::portals::clear_on_map_load))
            .add_systems(Startup, save_info)
            .add_plugins(autosave::AutosavePlugin);
        let mut reg = SaveRegistry::default();
        registry::register_all(&mut reg);
        app.insert_resource(reg);
        let mut c = app.world_mut().resource_mut::<Console>();
        c.register_command("save", save_command);
        c.register_command("load", load_command);
        c.register_command("quicksave", |w, _| save_named(w, "quick", true));
        c.register_command("quickload", |w, _| load_named(w, "quick"));
        c.register_command("autosave", |w, _| save_named(w, "autosave", true));
        c.register_command("autosavedangerous", |w, _| save_named(w, "autosavedangerous", false));
        c.register_command("autosavedangerousissafe", |w, _| commit_dangerous(w));
        c.register_command("reload", reload_command);
        c.register_command("load_latest", |w, _| {
            if let Some(name) = newest_save() {
                load_named(w, &name);
            }
        });
    }
}

/// The most recent save written or loaded (what `reload` loads).
#[derive(Resource, Default)]
pub struct SaveState {
    pub last: Option<String>,
}

/// A saved game being restored: waits for its map to load.
#[derive(Resource)]
pub struct PendingRestore {
    pub file: SaveFile,
    /// Set once the entities are back; the player follows after the player
    /// placement of the level load.
    pub entities_done: bool,
}

#[derive(Serialize, Deserialize)]
pub struct SaveFile {
    pub version: u32,
    pub map: String,
    pub comment: String,
    /// Seconds since 1970.
    pub saved_at: u64,
    pub tick: u64,
    pub curtime: f64,
    pub next_id: u32,
    pub entities: Vec<SavedEntity>,
    pub resources: Vec<(String, Vec<u8>)>,
    pub scripts: Option<Vec<u8>>,
    pub player: Option<SavedPlayer>,
}

#[derive(Serialize, Deserialize)]
pub struct SavedEntity {
    pub id: u32,
    pub index: Option<u32>,
    pub classname: String,
    pub targetname: Option<String>,
    pub keyvalues: Vec<(String, String)>,
    pub outputs: Vec<Connection>,
    pub transform: Transform,
    /// Parent entity id and, for a model's bone, the bone index.
    pub parent: Option<(u32, Option<usize>)>,
    pub hidden: bool,
    pub components: Vec<(String, Vec<u8>)>,
}

#[derive(Serialize, Deserialize)]
pub struct SavedPlayer {
    pub state: crate::player::movement::MoveState,
    pub angles: Vec3,
    pub noclip: bool,
    pub health: Option<crate::player::health::Health>,
}

/// Our save folder: `%LOCALAPPDATA%/portal2rust/save` (or the XDG data
/// folder elsewhere).
pub fn save_dir() -> PathBuf {
    std::env::var_os("P2R_SAVE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("portal2rust").join("save")))
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(|d| PathBuf::from(d).join("portal2rust").join("save")))
        .or_else(|| std::env::var_os("HOME").map(|d| PathBuf::from(d).join(".local/share/portal2rust/save")))
        .unwrap_or_else(|| PathBuf::from("save"))
}

fn path_for(name: &str) -> PathBuf {
    let name = name.trim().trim_end_matches(".sav");
    save_dir().join(format!("{name}.sav"))
}

/// The most recently written save's name (without `.sav`).
pub fn newest_save() -> Option<String> {
    let dir = std::fs::read_dir(save_dir()).ok()?;
    dir.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("sav")))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(t, _)| *t)
        .and_then(|(_, p)| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
}

/// The map of the most recent save, if any.
pub fn newest_save_map() -> Option<String> {
    read_file(&newest_save()?).ok().map(|f| f.map)
}

pub fn write_file(name: &str, file: &SaveFile) -> Result<PathBuf, String> {
    let path = path_for(name);
    std::fs::create_dir_all(save_dir()).map_err(|e| e.to_string())?;
    let bytes = encode(file)?;
    let tmp = path.with_extension("tmp");
    {
        let f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        gz.write_all(&bytes).map_err(|e| e.to_string())?;
        gz.finish().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn read_file(name: &str) -> Result<SaveFile, String> {
    let path = path_for(name);
    let f = std::fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(f).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let file: SaveFile = decode(&bytes)?;
    if file.version != VERSION {
        return Err(format!("save version {} (expected {VERSION})", file.version));
    }
    Ok(file)
}

fn print(world: &mut World, text: impl Into<String>) {
    world.resource_mut::<Console>().print(text);
}

fn save_command(world: &mut World, args: &CommandArgs) {
    let name = args.arg(1).to_owned();
    if name.is_empty() {
        print(world, "save <savename>");
        return;
    }
    // Quick saves and autosaves keep their previous copy.
    let history = matches!(name.to_ascii_lowercase().as_str(), "quick" | "autosave");
    save_named(world, &name, history);
}

fn load_command(world: &mut World, args: &CommandArgs) {
    let name = args.arg(1).to_owned();
    if name.is_empty() {
        print(world, "load <savename>");
        return;
    }
    load_named(world, &name);
}

/// Saves as `name`; with `history` the previous file of that name is kept as
/// `<name>01`.
pub fn save_named(world: &mut World, name: &str, history: bool) {
    if world.get_resource::<PendingRestore>().is_some() {
        return;
    }
    let dead =
        world.resource::<EntityRegistry>().player.and_then(|p| world.get::<crate::player::health::Health>(p)).is_some_and(|h| h.dead);
    if dead {
        print(world, "Can't savegame with a dead player");
        return;
    }
    let file = match capture(world, name) {
        Ok(f) => f,
        Err(e) => {
            print(world, format!("Save failed: {e}"));
            return;
        }
    };
    if history {
        let (from, to) = (path_for(name), path_for(&format!("{name}01")));
        if from.exists() {
            let _ = std::fs::rename(&from, &to);
        }
    }
    match write_file(name, &file) {
        Ok(path) => {
            info!("saved {} ({} entities)", path.display(), file.entities.len());
            world.resource_mut::<SaveState>().last = Some(name.to_owned());
        }
        Err(e) => print(world, format!("Save failed: {e}")),
    }
}

/// A dangerous autosave that turned out safe becomes the autosave.
fn commit_dangerous(world: &mut World) {
    let (from, to) = (path_for("autosavedangerous"), path_for("autosave"));
    if !from.exists() {
        return;
    }
    if to.exists() {
        let _ = std::fs::rename(&to, path_for("autosave01"));
    }
    if std::fs::rename(&from, &to).is_ok() {
        world.resource_mut::<SaveState>().last = Some("autosave".into());
    }
}

pub fn load_named(world: &mut World, name: &str) {
    match read_file(name) {
        Ok(file) => {
            info!("loading {name} ({})", file.map);
            world.resource_mut::<SaveState>().last = Some(name.to_owned());
            let map = file.map.clone();
            world.insert_resource(PendingRestore { file, entities_done: false });
            world.write_message(LoadMap::new(map, MapLoadType::LoadGame));
            world.resource_mut::<Console>().wait_for_map();
        }
        Err(e) => print(world, format!("Couldn't load {name}: {e}")),
    }
}

/// `reload`: the most recent save, or the current map from the start.
fn reload_command(world: &mut World, _args: &CommandArgs) {
    let last = world.resource::<SaveState>().last.clone().filter(|n| path_for(n).exists());
    match last {
        Some(name) => load_named(world, &name),
        None => {
            if let Some(map) = world.get_resource::<CurrentMap>().map(|m| m.name.clone()) {
                world.write_message(LoadMap::new(map, MapLoadType::NewGame));
            }
        }
    }
}

fn joint_owners(world: &mut World) -> HashMap<Entity, (u32, usize)> {
    let mut q = world.query::<(&SaveId, &StudioAnimator)>();
    let mut out = HashMap::new();
    for (id, a) in q.iter(world) {
        for (bone, &j) in a.joints.iter().enumerate() {
            out.insert(j, (id.0, bone));
        }
    }
    out
}

/// Everything about the current game, as a save.
pub fn capture(world: &mut World, comment: &str) -> Result<SaveFile, String> {
    let map = world.get_resource::<CurrentMap>().ok_or("no map loaded")?.name.clone();
    let mut ids: HashMap<Entity, u32> = HashMap::new();
    let mut q = world.query::<(Entity, &SaveId)>();
    for (e, id) in q.iter(world) {
        ids.insert(e, id.0);
    }
    let player = world.resource::<EntityRegistry>().player;
    if let Some(p) = player {
        ids.insert(p, PLAYER_ID);
    }
    let joints = joint_owners(world);
    let (components, resources) = {
        let r = world.resource::<SaveRegistry>();
        (r.components.clone(), r.resources.clone())
    };
    let scripts = crate::script::save::capture(world, &ids)?;
    let mut rows: Vec<(Entity, u32)> = {
        let mut q = world.query_filtered::<(Entity, &SaveId), With<MapEntity>>();
        q.iter(world).map(|(e, id)| (e, id.0)).collect()
    };
    rows.sort_by_key(|r| r.1);
    let clock = *world.resource::<GameClock>();
    let next_id = world.resource::<ids::NextSaveId>().0;
    ids::saving(ids.clone(), || {
        let mut entities = Vec::with_capacity(rows.len());
        for (e, id) in rows {
            let Some(me) = world.get::<MapEntity>(e) else { continue };
            let parent = world.get::<ChildOf>(e).map(|c| c.parent()).and_then(|p| match ids.get(&p) {
                Some(&pid) if pid != PLAYER_ID => Some((pid, None)),
                _ => joints.get(&p).map(|&(owner, bone)| (owner, Some(bone))),
            });
            let mut comps = Vec::new();
            for (name, save, _) in &components {
                if let Some(bytes) = save(world, e) {
                    comps.push((name.to_string(), bytes?));
                }
            }
            entities.push(SavedEntity {
                id,
                index: (me.index < ids::RUNTIME_IDS as usize).then_some(me.index as u32),
                classname: me.classname.clone(),
                targetname: me.targetname.clone(),
                keyvalues: me.keyvalues.clone(),
                outputs: world.get::<Outputs>(e).map(|o| o.connections.clone()).unwrap_or_default(),
                transform: world.get::<Transform>(e).copied().unwrap_or_default(),
                parent,
                hidden: world.get::<Visibility>(e) == Some(&Visibility::Hidden),
                components: comps,
            });
        }
        let mut res = Vec::new();
        for (name, save, _) in &resources {
            if let Some(bytes) = save(world) {
                res.push((name.to_string(), bytes?));
            }
        }
        let player = player.and_then(|p| {
            let pl = world.get::<Player>(p)?;
            Some(SavedPlayer {
                state: pl.state,
                angles: world.resource::<crate::player::PlayerInput>().input.angles,
                noclip: pl.noclip,
                health: world.get::<crate::player::health::Health>(p).cloned(),
            })
        });
        Ok(SaveFile {
            version: VERSION,
            map,
            comment: comment.to_owned(),
            saved_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
            tick: clock.tick,
            curtime: clock.curtime,
            next_id,
            entities,
            resources: res,
            scripts,
            player,
        })
    })
}

/// Spawns the saved entities of the map that just loaded and restores their
/// state. Returns false when no saved game is waiting for this map.
pub fn restore_map_entities(world: &mut World) -> bool {
    let Some(map) = world.get_resource::<CurrentMap>().map(|m| m.name.clone()) else { return false };
    let Some(pending) = world.remove_resource::<PendingRestore>() else { return false };
    if pending.file.map != map || pending.entities_done {
        world.insert_resource(pending);
        return false;
    }
    let file = pending.file;
    if let Err(e) = restore(world, &file) {
        error!("restoring the saved game failed: {e}");
    }
    world.insert_resource(PendingRestore { file, entities_done: true });
    true
}

fn restore(world: &mut World, file: &SaveFile) -> Result<(), String> {
    *world.resource_mut::<GameClock>() = GameClock { tick: file.tick, curtime: file.curtime };
    world.resource_mut::<EventQueue>().now = file.curtime;
    world.resource_mut::<ids::NextSaveId>().0 = file.next_id;
    crate::script::init_vm(world);
    let player = crate::player::ensure_player(world);

    let mut by_id: HashMap<u32, Entity> = HashMap::new();
    by_id.insert(PLAYER_ID, player);
    let mut spawned = Vec::with_capacity(file.entities.len());
    for s in &file.entities {
        let defaults = world.resource_mut::<EntityDefinitions>().class(&s.classname);
        let index = s.index.map_or(usize::MAX, |i| i as usize);
        let outputs = Outputs { connections: s.outputs.clone() };
        let e = crate::entities::insert_entity(
            world,
            index,
            SaveId(s.id),
            &s.classname,
            s.targetname.clone(),
            s.keyvalues.clone(),
            outputs,
            defaults,
        );
        by_id.insert(s.id, e);
        spawned.push((e, s.classname.clone()));
    }
    crate::entities::setup_entities_with(world, &spawned, true);
    // Spawn code may queue commands (placement included); apply them before
    // the saved state goes on top.
    world.flush();
    // Entities the spawn code made itself are already in the save.
    let saved: std::collections::HashSet<Entity> = by_id.values().copied().collect();
    let extra: Vec<Entity> = {
        let mut q = world.query_filtered::<Entity, With<MapEntity>>();
        q.iter(world).filter(|e| !saved.contains(e)).collect()
    };
    for e in extra {
        crate::entities::despawn_entity(world, e);
    }

    // Placement and parents, then component state.
    for s in &file.entities {
        let Some(&e) = by_id.get(&s.id) else { continue };
        let parent = s.parent.and_then(|(pid, bone)| {
            let p = *by_id.get(&pid)?;
            match bone {
                None => Some(p),
                Some(b) => world.get::<StudioAnimator>(p).and_then(|a| a.joints.get(b).copied()),
            }
        });
        let mut em = world.entity_mut(e);
        match parent {
            Some(p) => {
                em.insert(ChildOf(p));
            }
            None => {
                em.remove::<ChildOf>();
            }
        }
        em.insert(s.transform);
        em.insert(if s.hidden { Visibility::Hidden } else { Visibility::Visible });
    }
    let (components, resources) = {
        let r = world.resource::<SaveRegistry>();
        (r.components.clone(), r.resources.clone())
    };
    let loaders: HashMap<&str, _> = components.iter().map(|(n, _, l)| (*n, *l)).collect();
    let res_loaders: HashMap<&str, _> = resources.iter().map(|(n, _, l)| (*n, *l)).collect();
    ids::loading(by_id.clone(), || {
        for s in &file.entities {
            let Some(&e) = by_id.get(&s.id) else { continue };
            for (name, bytes) in &s.components {
                match loaders.get(name.as_str()) {
                    Some(load) => {
                        if let Err(err) = load(world, e, bytes) {
                            warn!("restoring {name} of {} {}: {err}", s.classname, s.id);
                        }
                    }
                    None => warn!("saved component {name} is unknown"),
                }
            }
        }
        for (name, bytes) in &file.resources {
            match res_loaders.get(name.as_str()) {
                Some(load) => {
                    if let Err(err) = load(world, bytes) {
                        warn!("restoring {name}: {err}");
                    }
                }
                None => warn!("saved resource {name} is unknown"),
            }
        }
    });
    if let Some(bytes) = &file.scripts {
        crate::script::save::restore(world, bytes, &by_id)?;
    }
    registry::after_restore(world);
    Ok(())
}

/// Puts the player back once the level load has placed them.
pub fn finish_restore(world: &mut World) {
    let done = world.get_resource::<PendingRestore>().is_some_and(|p| p.entities_done);
    if !done {
        return;
    }
    let Some(pending) = world.remove_resource::<PendingRestore>() else { return };
    let Some(saved) = pending.file.player else { return };
    let Some(p) = world.resource::<EntityRegistry>().player else { return };
    if let Some(mut pl) = world.get_mut::<Player>(p) {
        pl.state = saved.state;
        // Found again by the next movement tick.
        pl.state.ground_entity = None;
        pl.previous_origin = saved.state.origin;
        pl.noclip = saved.noclip;
        pl.ground_ref = None;
    }
    if let Some(mut t) = world.get_mut::<Transform>(p) {
        t.translation = saved.state.origin;
    }
    world.resource_mut::<crate::player::PlayerInput>().input.angles = saved.angles;
    if let Some(h) = saved.health {
        world.entity_mut(p).insert(h);
    }
}

/// `--save-info <name>`: what a saved game holds, then quits.
fn save_info(world: &mut World) {
    let Some(name) = world.resource::<portal2_core::args::Args>().save_info.clone() else { return };
    match read_file(&name) {
        Ok(f) => {
            println!(
                "{name}: {} at tick {} ({:.2} s), {} entities, scripts {} bytes",
                f.map,
                f.tick,
                f.curtime,
                f.entities.len(),
                f.scripts.as_ref().map_or(0, |s| s.len())
            );
            for e in &f.entities {
                for (c, bytes) in &e.components {
                    if c == "choreo_scene"
                        && let Ok(scene) = decode::<crate::entities::scenes::ChoreoScene>(bytes)
                    {
                        let (file, playing, time) = crate::entities::scenes::describe(&scene);
                        if playing {
                            println!("  scene {} {file}: playing at {time:.2} s", e.id);
                        }
                    }
                }
            }
            if let Some((_, bytes)) = f.resources.iter().find(|(n, _)| n == "event_queue") {
                println!("  event queue {} bytes", bytes.len());
            }
            if let Some(p) = &f.player {
                println!("  player at {:?}, angles {:?}", p.state.origin, p.angles);
            }
        }
        Err(e) => println!("{name}: {e}"),
    }
    world.write_message(AppExit::Success);
}
