//! Histogram of MDL / VVD / VTX header versions across the install.
use std::collections::BTreeMap;
use source_formats::fs::GameFileSystem;

fn main() {
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let models = fs.list("models", Some("mdl"), true);
    let mut mdl_versions = BTreeMap::new();
    let mut vtx_versions = BTreeMap::new();
    let mut vvd_versions = BTreeMap::new();
    let mut vtx_kinds = BTreeMap::new();
    for m in &models {
        let Some(d) = fs.read(m) else { continue };
        let v = i32::from_le_bytes(d[4..8].try_into().unwrap());
        *mdl_versions.entry(v).or_insert(0) += 1;
        let base = m.trim_end_matches(".mdl");
        for ext in ["dx90.vtx", "vtx"] {
            if let Some(x) = fs.read(&format!("{base}.{ext}")) {
                *vtx_versions.entry((ext, i32::from_le_bytes(x[0..4].try_into().unwrap()))).or_insert(0) += 1;
                *vtx_kinds.entry(ext).or_insert(0) += 1;
                break;
            }
        }
        if let Some(x) = fs.read(&format!("{base}.vvd")) {
            *vvd_versions.entry(i32::from_le_bytes(x[4..8].try_into().unwrap())).or_insert(0) += 1;
        }
    }
    println!("{} models; mdl versions {mdl_versions:?}; vtx {vtx_versions:?}; vvd {vvd_versions:?}", models.len());
}
