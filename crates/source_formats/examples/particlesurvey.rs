//! Which particle systems the singleplayer maps start, and which operators
//! those systems (with their children) need: `particlesurvey [map ...]`.
//! System names may also be passed with `--system <name>` to include effects
//! started by code rather than by map entities; `--names` lists every
//! system in the manifest files instead.
use std::collections::{BTreeMap, BTreeSet};

use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;
use source_formats::pcf::{self, OperatorKind, Pcf};

fn main() {
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let mut maps: Vec<String> = Vec::new();
    let mut extra: Vec<String> = Vec::new();
    let mut names_only = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--names" {
            names_only = true;
        } else if a == "--system" {
            extra.extend(args.next());
        } else {
            maps.push(a);
        }
    }
    if maps.is_empty() {
        maps = fs
            .list("maps", Some("bsp"), false)
            .into_iter()
            .filter_map(|p| {
                let name = p.rsplit('/').next()?.trim_end_matches(".bsp").to_owned();
                name.starts_with("sp_").then_some(name)
            })
            .collect();
        maps.sort();
    }

    // Manifest files only: those are the definitions the game knows by name.
    let manifest = fs.read_string(pcf::MANIFEST_PATH).map(|t| pcf::parse_manifest(&t)).unwrap_or_default();
    let mut files: Vec<(String, Pcf)> = Vec::new();
    for e in &manifest {
        if files.iter().any(|(p, _)| *p == e.path) {
            continue;
        }
        if let Some(pcf) = fs.read(&e.path).and_then(|d| Pcf::parse(&d).ok()) {
            files.push((e.path.clone(), pcf));
        }
    }
    if names_only {
        for (path, pcf) in &files {
            for s in &pcf.systems {
                println!("{}	{path}", s.name);
            }
        }
        return;
    }
    let find = |name: &str| {
        files
            .iter()
            .find_map(|(path, pcf)| pcf.systems.iter().position(|s| s.name.eq_ignore_ascii_case(name)).map(|i| (path.as_str(), pcf, i)))
    };

    let mut used: BTreeMap<String, usize> = BTreeMap::new();
    for map in &maps {
        let Some(bytes) = fs.read(&format!("maps/{map}.bsp")) else { continue };
        let Ok(bsp) = Bsp::parse(&bytes) else { continue };
        let mut names = BTreeSet::new();
        for e in &bsp.entities {
            if e.get("classname") != Some("info_particle_system") {
                continue;
            }
            if let Some(n) = e.get("effect_name") {
                names.insert(n.to_ascii_lowercase());
                *used.entry(n.to_ascii_lowercase()).or_default() += 1;
            }
        }
        println!("{map:26} {}", names.into_iter().collect::<Vec<_>>().join(" "));
    }
    for n in extra {
        *used.entry(n.to_ascii_lowercase()).or_default() += 1;
    }

    // Close over children.
    let mut systems: BTreeSet<(String, usize)> = BTreeSet::new();
    let mut missing = BTreeSet::new();
    let mut stack: Vec<String> = used.keys().cloned().collect();
    while let Some(name) = stack.pop() {
        let Some((path, pcf, i)) = find(&name) else {
            missing.insert(name);
            continue;
        };
        if !systems.insert((path.to_owned(), i)) {
            continue;
        }
        for c in &pcf.systems[i].children {
            stack.push(pcf.systems[c.index].name.clone());
        }
    }
    println!("\n{} systems started by maps, {} with children; missing {:?}", used.len(), systems.len(), missing);

    let mut census: BTreeMap<(OperatorKind, String), usize> = BTreeMap::new();
    let mut materials: BTreeMap<String, usize> = BTreeMap::new();
    for (path, i) in &systems {
        let pcf = &files.iter().find(|(p, _)| p == path).unwrap().1;
        let def = &pcf.systems[*i];
        *materials.entry(def.material.to_ascii_lowercase()).or_default() += 1;
        for kind in OperatorKind::ALL {
            for op in def.stack(kind) {
                *census.entry((kind, op.function_name.to_ascii_lowercase())).or_default() += 1;
            }
        }
    }
    for kind in OperatorKind::ALL {
        println!("\n{}:", kind.attribute());
        let mut rows: Vec<_> = census.iter().filter(|((k, _), _)| *k == kind).collect();
        rows.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        for ((_, name), n) in rows {
            println!("  {n:4} {name}");
        }
    }
    println!("\nmaterials: {}", materials.len());
    for (m, n) in &materials {
        println!("  {n:4} {m}");
    }
}
