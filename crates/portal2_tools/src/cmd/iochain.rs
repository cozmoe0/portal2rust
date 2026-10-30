//! Walks the I/O graph backwards from every entity of a class: who parents
//! it and which entities send it inputs, a few levels up. Prints entity
//! indices, classnames, positions and output/input names only.
use std::collections::BTreeSet;

use source_formats::bsp::Bsp;

pub fn main() {
    let mut args = crate::args().skip(1);
    let map = args.next().unwrap_or_else(|| "sp_a2_laser_intro".to_owned());
    let class = args.next().unwrap_or_else(|| "env_portal_laser".to_owned());
    let depth: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(3);
    let fs = crate::game_fs();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let ents = &bsp.entities;
    let name = |i: usize| ents[i].get("targetname").map(str::to_ascii_lowercase);
    let matches = |pattern: &str, i: usize| {
        let Some(n) = name(i) else { return false };
        let p = pattern.to_ascii_lowercase();
        match p.strip_suffix('*') {
            Some(prefix) => n.starts_with(prefix),
            None => n == p,
        }
    };
    let describe = |i: usize| format!("#{i} {} at {:?}", ents[i].classname(), ents[i].origin());

    let mut frontier: BTreeSet<usize> = (0..ents.len()).filter(|&i| ents[i].classname() == class).collect();
    for &i in &frontier {
        println!("{}", describe(i));
        if let Some(parent) = ents[i].get("parentname") {
            let parent = parent.split(',').next().unwrap_or(parent);
            for j in (0..ents.len()).filter(|&j| matches(parent, j)) {
                println!("  parent {}", describe(j));
            }
        }
    }
    let mut seen = frontier.clone();
    for level in 1..=depth {
        let mut next = BTreeSet::new();
        for (s, e) in ents.iter().enumerate() {
            for (key, value) in &e.pairs {
                let fields: Vec<&str> = if value.contains('\u{1b}') { value.split('\u{1b}').collect() } else { value.split(',').collect() };
                if fields.len() < 2 || !key.to_ascii_lowercase().starts_with("on") {
                    continue;
                }
                for &t in frontier.iter().filter(|&&t| matches(fields[0], t)) {
                    let delay = fields.get(3).copied().unwrap_or("0");
                    println!("{level}: {} --{key}--> #{t} {} (delay {delay})", describe(s), fields[1]);
                    if seen.insert(s) {
                        next.insert(s);
                    }
                }
            }
        }
        frontier = next;
        if frontier.is_empty() {
            break;
        }
    }
}
