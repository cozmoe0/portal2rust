//! Survey for actor animation (names and counts only): pose parameters,
//! flexes, eyeballs and mouths on the models SP maps use; which actors scenes
//! drive with look-at, face, flex animation and expression events; and which
//! voice waves carry phoneme data.
//!
//! `cargo run --release -p source_formats --example actorscan [models|scenes|waves]`
use std::collections::{BTreeMap, BTreeSet};

use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;
use source_formats::mdl::Mdl;
use source_formats::scenes_image::ScenesImage;
use source_formats::vcd::EventKind;
use source_formats::wav::{SentenceData, Wave};

fn i32_at(d: &[u8], o: usize) -> i32 {
    d.get(o..o + 4).map_or(0, |b| i32::from_le_bytes(b.try_into().unwrap()))
}

fn cstr(d: &[u8], o: usize) -> String {
    let s = d.get(o..).unwrap_or(&[]);
    String::from_utf8_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())]).into_owned()
}

fn main() {
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let what = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    if what == "find" {
        // `find <substring>...`: every installed model whose path contains one.
        let subs: Vec<String> = std::env::args().skip(2).map(|s| s.to_ascii_lowercase()).collect();
        let mut users = BTreeMap::new();
        for m in fs.list("models", Some("mdl"), true) {
            if subs.iter().any(|s| m.to_ascii_lowercase().contains(s.as_str())) {
                users.insert(m, BTreeSet::from(["find".to_owned()]));
            }
        }
        report(&fs, &users, true);
        return;
    }
    if what == "ents" {
        // `ents <substring>`: SP entities whose class or keys contain it.
        let needle = std::env::args().nth(2).unwrap_or_default().to_ascii_lowercase();
        for m in fs.list("maps", Some("bsp"), false).into_iter().filter(|m| m.contains("/sp_")) {
            let Some(data) = fs.read(&m) else { continue };
            let Ok(bsp) = Bsp::parse(&data) else { continue };
            for e in &bsp.entities {
                let hit = e.classname().to_ascii_lowercase().contains(&needle)
                    || e.pairs.iter().any(|(k, v)| k.to_ascii_lowercase().contains(&needle) || v.to_ascii_lowercase().contains(&needle));
                if hit {
                    let keys: Vec<String> = e.pairs.iter().map(|(k, v)| format!("{k}={}", v.replace('\u{1b}', ","))).collect();
                    println!("{m}: {}", keys.join(" | "));
                }
            }
        }
        return;
    }
    if what == "all" || what == "models" {
        models(&fs);
    }
    if what == "all" || what == "scenes" {
        scenes(&fs);
    }
    if what == "all" || what == "waves" {
        waves(&fs);
    }
}

fn models(fs: &GameFileSystem) {
    let maps: Vec<String> = fs.list("maps", Some("bsp"), false).into_iter().filter(|m| m.contains("/sp_")).collect();
    let mut users: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for m in &maps {
        let Some(data) = fs.read(m) else { continue };
        let Ok(bsp) = Bsp::parse(&data) else { continue };
        for e in &bsp.entities {
            let class = e.classname().to_owned();
            let model = match e.get("model") {
                Some(m) if m.to_ascii_lowercase().ends_with(".mdl") => m.to_ascii_lowercase().replace('\\', "/"),
                _ if class == "npc_personality_core" => "models/npcs/personality_sphere/personality_sphere_skins.mdl".into(),
                _ => continue,
            };
            users.entry(model).or_default().insert(class);
        }
    }
    for (m, c) in [("models/player/chell/player.mdl", "player"), ("models/weapons/v_portalgun.mdl", "viewmodel")] {
        users.entry(m.into()).or_default().insert(c.into());
    }
    report(fs, &users, false);
}

fn report(fs: &GameFileSystem, users: &BTreeMap<String, BTreeSet<String>>, all: bool) {
    let mut plain = 0;
    for (model, classes) in users {
        let Some(d) = fs.read(model) else { continue };
        let Ok(mdl) = Mdl::parse(d.clone()) else { continue };
        let (n_flexdesc, n_ctrl, n_rules, n_mouths) = (i32_at(&d, 260), i32_at(&d, 268), i32_at(&d, 276), i32_at(&d, 292));
        let (mut eyeballs, mut flexes) = (0, 0);
        let (n_bp, bp_index) = (i32_at(&d, 232), i32_at(&d, 236) as usize);
        for b in 0..n_bp.max(0) as usize {
            let bp = bp_index + b * 16;
            let (n_models, model_index) = (i32_at(&d, bp + 4), i32_at(&d, bp + 12) as usize);
            for m in 0..n_models.max(0) as usize {
                let mb = bp + model_index + m * 148;
                eyeballs += i32_at(&d, mb + 100);
                let (n_meshes, mesh_index) = (i32_at(&d, mb + 72), i32_at(&d, mb + 76) as usize);
                for k in 0..n_meshes.max(0) as usize {
                    flexes += i32_at(&d, mb + mesh_index + k * 116 + 16);
                }
            }
        }
        let autolayered = (0..mdl.sequences.len()).filter(|&i| i32_at(&d, i32_at(&d, 192) as usize + i * 212 + 148) > 0).count();
        let blended = mdl.sequences.iter().filter(|s| s.group_size[0] > 1 || s.group_size[1] > 1).count();
        if !all && mdl.pose_parameters.is_empty() && n_flexdesc == 0 && eyeballs == 0 && autolayered == 0 {
            plain += 1;
            continue;
        }
        // studiohdr2: bone flex drivers.
        let hdr2 = i32_at(&d, 400) as usize;
        let flex_drivers = if hdr2 > 0 { i32_at(&d, hdr2 + 24) } else { 0 };
        let ctrl_index = i32_at(&d, 272) as usize;
        let ctrls: Vec<String> = (0..n_ctrl.max(0) as usize)
            .map(|i| {
                let c = ctrl_index + i * 20;
                cstr(&d, c + i32_at(&d, c + 4) as usize)
            })
            .collect();
        let poses: Vec<String> = mdl.pose_parameters.iter().map(|p| format!("{}[{}..{}]", p.name, p.start, p.end)).collect();
        println!("{model}  ({})", classes.iter().cloned().collect::<Vec<_>>().join(", "));
        println!(
            "    pose {poses:?}; seqs {} (blend grids {blended}, autolayers {autolayered}); includes {:?}",
            mdl.sequences.len(),
            mdl.include_models
        );
        if n_flexdesc > 0 || eyeballs > 0 || flex_drivers > 0 {
            println!(
                "    flexdesc {n_flexdesc}, controllers {n_ctrl}, rules {n_rules}, mesh flexes {flexes}, eyeballs {eyeballs}, mouths {n_mouths}, bone flex drivers {flex_drivers}"
            );
            println!("    controllers {ctrls:?}");
        }
    }
    println!("{} models, {plain} with no pose parameters, flexes, eyeballs or autolayers", users.len());
}

fn scenes(fs: &GameFileSystem) {
    let Some(Ok(image)) = ScenesImage::from_fs(fs) else { return };
    let mut kinds: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut tracks: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut targets: BTreeMap<(String, String), BTreeMap<String, usize>> = BTreeMap::new();
    let mut ignore_phonemes = 0;
    for i in 0..image.len() {
        let Ok(scene) = image.scene(i) else { continue };
        ignore_phonemes += scene.ignore_phonemes as usize;
        for a in &scene.actors {
            let actor = a.name.to_ascii_lowercase();
            for c in &a.channels {
                for e in &c.events {
                    let kind = e.kind.name().to_owned();
                    *kinds.entry((actor.clone(), kind.clone())).or_default() += 1;
                    match e.kind {
                        EventKind::Gesture if std::env::var("GESTURES").is_ok() => {
                            let p = format!("gesture {}", e.param.to_ascii_lowercase());
                            *targets.entry((actor.clone(), "gesture".into())).or_default().entry(p).or_default() += 1;
                        }
                        EventKind::FlexAnimation => {
                            for t in &e.flex_tracks {
                                *tracks.entry(actor.clone()).or_default().entry(t.name.clone()).or_default() += 1;
                            }
                        }
                        EventKind::LookAt | EventKind::Face | EventKind::Expression => {
                            let p = format!("{} {}", e.param, e.param2).trim().to_ascii_lowercase();
                            *targets.entry((actor.clone(), kind)).or_default().entry(p).or_default() += 1;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    println!("{} scenes, {ignore_phonemes} with ignorePhonemes", image.len());
    for ((actor, kind), n) in &kinds {
        if !matches!(kind.as_str(), "speak" | "gesture" | "sequence" | "firetrigger") || *n > 0 {
            println!("  {n:>6} {kind:<14} {actor}");
        }
    }
    for (actor, t) in &tracks {
        println!("  flex tracks on {actor}: {t:?}");
    }
    for ((actor, kind), t) in &targets {
        let mut v: Vec<_> = t.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        println!("  {kind} on {actor}: {:?}", v.iter().take(8).collect::<Vec<_>>());
    }
}

fn waves(fs: &GameFileSystem) {
    let paths = fs.list("sound/vo", Some("wav"), true);
    let mut by_dir: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    let mut phonemes: BTreeMap<String, usize> = BTreeMap::new();
    let mut raw_vdat = 0;
    for p in &paths {
        let dir = p.split('/').take(3).collect::<Vec<_>>().join("/");
        let entry = by_dir.entry(dir).or_default();
        entry.0 += 1;
        let Some(d) = fs.read(p) else { continue };
        raw_vdat += d.windows(4).any(|w| w == b"VDAT") as usize;
        let Ok(w) = Wave::parse_header(&d) else { continue };
        match &w.sentence {
            Some(SentenceData::Parsed(s)) => {
                entry.1 += 1;
                for ph in s.words.iter().flat_map(|w| &w.phonemes) {
                    *phonemes.entry(ph.name.clone()).or_default() += 1;
                }
            }
            Some(SentenceData::Raw(_)) => entry.2 += 1,
            None => {}
        }
    }
    println!("{} voice waves (dir: total, with parsed sentence, with other VDAT); 'VDAT' bytes anywhere in {raw_vdat}", paths.len());
    for (dir, (n, s, r)) in &by_dir {
        println!("  {dir}: {n}, {s}, {r}");
    }
    println!("  phonemes: {phonemes:?}");
}
