//! Lists particle systems from the install's `.pcf` files with their operator
//! function names. Usage: `pcfdump` (every system, by file), `pcfdump <system>`
//! (properties, operator attributes and children of matching systems) or
//! `pcfdump --census` (functionName usage counts as markdown tables).

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use source_formats::fs::GameFileSystem;
use source_formats::pcf::{self, OperatorKind, ParticleSystemDefinition, Pcf};

pub fn main() {
    let arg = crate::args().nth(1);
    let root = crate::install_root();
    let fs = GameFileSystem::portal2(&root).expect("mount");
    let manifest = fs.read_string(pcf::MANIFEST_PATH).map(|t| pcf::parse_manifest(&t)).unwrap_or_default();
    let listed: BTreeSet<String> = manifest.iter().map(|e| e.path.clone()).collect();

    // Manifest order first, then every other packed file.
    let mut paths: Vec<String> = Vec::new();
    for path in manifest.iter().map(|e| e.path.clone()).chain(fs.list("particles", Some("pcf"), true)) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    let mut files = Vec::new();
    for path in paths {
        let Some(data) = fs.read(&path) else {
            eprintln!("{path}: missing");
            continue;
        };
        match Pcf::parse(&data) {
            Ok(pcf) => files.push((path, pcf)),
            Err(e) => eprintln!("{path}: {e}"),
        }
    }

    match arg.as_deref() {
        Some("--census") => census(&files, &listed),
        Some(name) => show(&files, name),
        None => list(&files, &listed),
    }
}

fn list(files: &[(String, Pcf)], listed: &BTreeSet<String>) {
    for (path, pcf) in files {
        let source = if listed.contains(path) { "manifest" } else { "not in manifest" };
        println!("== {path} ({} systems, {source})", pcf.systems.len());
        for def in &pcf.systems {
            println!("{}", def.name);
            for kind in OperatorKind::ALL {
                let ops = def.stack(kind);
                if !ops.is_empty() {
                    let names: Vec<&str> = ops.iter().map(|op| op.function_name.as_str()).collect();
                    println!("    {}: {}", kind.attribute(), names.join(", "));
                }
            }
            if !def.children.is_empty() {
                let children: Vec<String> = def.children.iter().map(child_label).collect();
                println!("    children: {}", children.join(", "));
            }
        }
    }
}

fn show(files: &[(String, Pcf)], name: &str) {
    let mut found = false;
    for (path, pcf) in files {
        let Some(def) = pcf.find(name) else { continue };
        found = true;
        println!("== {} in {path}", def.name);
        println!(
            "max {} initial {} radius {} color {:?} material {:?}",
            def.max_particles, def.initial_particles, def.radius, def.color, def.material
        );
        println!(
            "bounds {} .. {} cull radius {} cp {} max draw distance {}",
            def.bounding_box_min, def.bounding_box_max, def.cull_radius, def.cull_control_point, def.max_draw_distance
        );
        println!("stored attributes (others take defaults):");
        for a in def.attributes.iter() {
            println!("    {} = {}", a.name, a.value);
        }
        for kind in OperatorKind::ALL {
            for op in def.stack(kind) {
                let label = if op.name == op.function_name { String::new() } else { format!(" ({:?})", op.name) };
                println!("{} {:?}{label}", kind.attribute(), op.function_name);
                for a in op.attributes.iter() {
                    println!("    {} = {}", a.name, a.value);
                }
            }
        }
        for child in &def.children {
            println!("child {}", child_label(child));
        }
    }
    if !found {
        eprintln!("no system named {name:?}");
    }
}

fn child_label(child: &pcf::ParticleChild) -> String {
    let mut s = child.name.clone();
    if child.delay != 0.0 {
        s += &format!(" +{}s", child.delay);
    }
    if child.end_cap_effect {
        s += " (end cap)";
    }
    s
}

/// functionName counts per stack over every system in every file, and over
/// systems from manifest-listed files only. Names are grouped ignoring case,
/// as the game resolves them; every spelling seen is shown.
fn census(files: &[(String, Pcf)], listed: &BTreeSet<String>) {
    let all: Vec<&ParticleSystemDefinition> = files.iter().flat_map(|(_, pcf)| &pcf.systems).collect();
    let manifest: Vec<&ParticleSystemDefinition> =
        files.iter().filter(|(path, _)| listed.contains(path)).flat_map(|(_, pcf)| &pcf.systems).collect();
    println!(
        "{} files ({} in the manifest), {} systems ({} from manifest files)\n",
        files.len(),
        files.iter().filter(|(path, _)| listed.contains(path)).count(),
        all.len(),
        manifest.len()
    );
    #[derive(Default)]
    struct Row<'a> {
        all: usize,
        manifest: usize,
        spellings: BTreeMap<&'a str, usize>,
    }
    for kind in OperatorKind::ALL {
        let mut rows: BTreeMap<String, Row> = BTreeMap::new();
        for (systems, manifest_only) in [(&all, false), (&manifest, true)] {
            for def in systems.iter() {
                for op in def.stack(kind) {
                    let row = rows.entry(op.function_name.to_lowercase()).or_default();
                    if manifest_only {
                        row.manifest += 1;
                    } else {
                        row.all += 1;
                        *row.spellings.entry(op.function_name.as_str()).or_default() += 1;
                    }
                }
            }
        }
        let mut rows: Vec<Row> = rows.into_values().collect();
        rows.sort_by_key(|r| Reverse((r.all, r.manifest)));
        let total: usize = rows.iter().map(|r| r.all).sum();
        println!("### `{}` ({} functions, {total} uses)\n", kind.attribute(), rows.len());
        println!("| functionName | all files | manifest files |");
        println!("| --- | ---: | ---: |");
        for row in rows {
            let mut spellings: Vec<(&str, usize)> = row.spellings.into_iter().collect();
            spellings.sort_by_key(|&(_, n)| Reverse(n));
            let names: Vec<&str> = spellings.into_iter().map(|(s, _)| s).collect();
            println!("| {} | {} | {} |", names.join(" / "), row.all, row.manifest);
        }
        println!();
    }
}
