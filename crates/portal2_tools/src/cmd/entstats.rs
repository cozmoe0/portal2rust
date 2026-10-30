//! Structural entity statistics across maps (counts only): which classes
//! carry a `model` keyvalue, and how I/O proxy connections are shaped.
use source_formats::bsp::Bsp;
use std::collections::BTreeMap;

pub fn main() {
    let fs = crate::game_fs();
    let maps: Vec<String> = fs.list("maps", Some("bsp"), false).into_iter().filter(|m| m.contains("/sp_")).collect();
    let mut model_key: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    let mut proxy_inputs: BTreeMap<String, u32> = BTreeMap::new();
    let mut proxy_outputs: BTreeMap<String, u32> = BTreeMap::new();
    let mut sep: BTreeMap<&str, u32> = BTreeMap::new();
    for m in &maps {
        let Ok(bsp) = Bsp::parse(&fs.read(m).unwrap()) else { continue };
        let proxies: std::collections::HashSet<String> = bsp
            .entities
            .iter()
            .filter(|e| e.classname() == "func_instance_io_proxy")
            .filter_map(|e| e.targetname().map(|s| s.to_lowercase()))
            .collect();
        for e in &bsp.entities {
            let c = e.classname();
            if c.starts_with("prop_") || c.starts_with("npc_") || c.starts_with("env_portal") || c.contains("laser") {
                let ent = model_key.entry(c.to_owned()).or_default();
                if e.get("model").is_some() { ent.0 += 1 } else { ent.1 += 1 }
            }
            for (k, v) in &e.pairs {
                let has_esc = v.contains('\x1b');
                if has_esc {
                    *sep.entry("esc").or_default() += 1
                } else if v.matches(',').count() == 4 && k.starts_with("On") {
                    *sep.entry("comma").or_default() += 1
                }
                if !has_esc {
                    continue;
                }
                let parts: Vec<&str> = v.split('\x1b').collect();
                if parts.len() < 2 {
                    continue;
                }
                if proxies.contains(&parts[0].to_lowercase()) {
                    *proxy_inputs.entry(parts[1].to_owned()).or_default() += 1;
                }
                if c == "func_instance_io_proxy" {
                    *proxy_outputs.entry(k.chars().filter(|c| !c.is_ascii_digit()).collect()).or_default() += 1;
                }
            }
        }
    }
    println!("{} sp maps; output separators {sep:?}", maps.len());
    println!("model keyvalue present/absent by class:");
    for (c, (y, n)) in &model_key {
        println!("  {c}: {y}/{n}");
    }
    println!("inputs sent to proxies: {proxy_inputs:?}");
    println!("proxy output names (digits stripped): {proxy_outputs:?}");
}
