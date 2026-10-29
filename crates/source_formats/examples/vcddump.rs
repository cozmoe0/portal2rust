//! Prints a choreographed scene. Usage: `vcddump <scene path> [--image]`, e.g.
//! `vcddump scenes/npc/glados/sp_a1_intro1_01.vcd` (the `scenes/` prefix and
//! `.vcd` suffix are optional). The text `.vcd` is used when the game has one;
//! `--image` (or a scene that only exists compiled) reads `scenes.image`.

use source_formats::fs::GameFileSystem;
use source_formats::scenes_image::ScenesImage;
use source_formats::vcd::{Curve, Event, EventKind, Scene};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut path = args.next().expect("usage: vcddump <scene path> [--image]").replace('\\', "/").to_lowercase();
    let force_image = args.any(|a| a == "--image");
    if !path.starts_with("scenes/") {
        path = format!("scenes/{path}");
    }
    if !path.ends_with(".vcd") {
        path.push_str(".vcd");
    }
    let root = GameFileSystem::find_install().expect("Portal 2 install not found (set PORTAL2_DIR)");
    let fs = GameFileSystem::portal2(&root).expect("mount");
    let image = ScenesImage::from_fs(&fs).map(|r| r.expect("scenes.image"));

    let compiled = image.as_ref().and_then(|img| img.find(&path).map(|i| (img, i)));
    if let Some((img, i)) = compiled {
        let s = img.summary(i).expect("summary");
        println!("image entry {i}: {} ms, last speak ends {} ms, sounds {:?}", s.duration_ms, s.last_speak_ms, s.sounds);
    }
    let text = if force_image { None } else { fs.read_string(&path) };
    let scene = match (text, compiled) {
        (Some(text), _) => {
            println!("{path}: text from {}", fs.locate(&path).unwrap_or_default());
            Scene::parse(&text).expect("parse")
        }
        (None, Some((img, i))) => {
            println!("{path}: compiled");
            img.scene(i).expect("decode")
        }
        (None, None) => panic!("{path} not found"),
    };
    print_scene(&scene);
}

fn print_scene(s: &Scene) {
    let on = |b: bool| if b { "on" } else { "off" };
    let fps = s.fps.map_or("-".to_owned(), |f| f.to_string());
    println!("fps {fps}, snap {}, ignorePhonemes {}, ends at {:.3}s", on(s.snap), on(s.ignore_phonemes), s.end_time());
    if let Some(map) = &s.map_name {
        println!("map {map:?}");
    }
    print_curve("scene ramp", &s.ramp, "");
    for e in &s.events {
        print_event(e, "");
    }
    for a in &s.actors {
        println!("actor {:?}{}", a.name, if a.active { "" } else { " (inactive)" });
        for c in &a.channels {
            println!("  channel {:?}{}", c.name, if c.active { "" } else { " (inactive)" });
            for e in &c.events {
                print_event(e, "    ");
            }
        }
    }
}

fn print_event(e: &Event, indent: &str) {
    let time = if e.has_duration() { format!("{:7.3}-{:<7.3}", e.start, e.end) } else { format!("{:7.3}        ", e.start) };
    let mut line = format!("{indent}{time} {} {:?}", e.kind.name(), e.name);
    for (key, value) in [("param", &e.param), ("param2", &e.param2), ("param3", &e.param3)] {
        if !value.is_empty() {
            line += &format!(" {key} {value:?}");
        }
    }
    let f = &e.flags;
    for (set, name) in [
        (f.fixed_length, "fixedlength"),
        (f.resume_condition, "resumecondition"),
        (f.lock_body_facing, "lockbodyfacing"),
        (f.force_short_movement, "forceshortmovement"),
        (f.sync_to_following_gesture, "synctofollowinggesture"),
        (f.play_over_script, "playoverscript"),
        (!f.active, "inactive"),
    ] {
        if set {
            line += &format!(" {name}");
        }
    }
    if e.distance_to_target != 0.0 {
        line += &format!(" distance {}", e.distance_to_target);
    }
    if e.pitch != 0 || e.yaw != 0 {
        line += &format!(" pitch {} yaw {}", e.pitch, e.yaw);
    }
    match e.kind {
        EventKind::Loop => line += &format!(" loopcount {}", e.loop_count),
        EventKind::Speak => line += &format!(" {} {:?}", e.cc.kind.name(), e.cc.token),
        _ => {}
    }
    if let Some(d) = e.sequence_duration {
        line += &format!(" sequence {d:.3}s");
    }
    println!("{line}");
    let indent = format!("{indent}        ");
    print_curve("ramp", &e.ramp, &indent);
    for (label, tags) in [("tags", &e.tags), ("playback tags", &e.playback_tags), ("shifted tags", &e.shifted_tags)] {
        if !tags.is_empty() {
            let list: Vec<_> = tags.iter().map(|t| format!("{}@{:.3}", t.name, t.value)).collect();
            println!("{indent}{label}: {}", list.join(" "));
        }
    }
    if !e.timing_tags.is_empty() {
        let list: Vec<_> = e.timing_tags.iter().map(|t| format!("{}@{:.3}", t.name, t.value)).collect();
        println!("{indent}timing tags: {}", list.join(" "));
    }
    if let Some(r) = &e.relative_tag {
        println!("{indent}starts at tag {:?} of {:?}", r.tag, r.wav);
    }
    for t in &e.flex_tracks {
        let samples: Vec<_> = t.samples.iter().map(|s| format!("{:.3}={:.3}", s.time, s.value)).collect();
        let combo = if t.combo { format!(" combo ({} balance samples)", t.balance.len()) } else { String::new() };
        println!("{indent}flex {:?} [{}, {}]{combo}: {}", t.name, t.min, t.max, samples.join(" "));
    }
}

fn print_curve(label: &str, c: &Curve, indent: &str) {
    if c.samples.is_empty() {
        return;
    }
    let samples: Vec<_> = c.samples.iter().map(|s| format!("{:.3}={:.3}", s.time, s.value)).collect();
    println!("{indent}{label}: {}", samples.join(" "));
}
