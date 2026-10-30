//! Lists the key structure of a KeyValues file (keys and short values only).
use source_formats::kv::{self, KeyValues};

fn walk(kv: &KeyValues, depth: usize) {
    if depth > 2 {
        return;
    }
    for c in kv.children() {
        match c.as_str() {
            Some(v) if v.len() <= 64 => println!("{}{} = {}", "  ".repeat(depth), c.key, v),
            Some(_) => println!("{}{} = <long>", "  ".repeat(depth), c.key),
            None => {
                println!("{}{} {{", "  ".repeat(depth), c.key);
                walk(c, depth + 1);
            }
        }
    }
}

pub fn main() {
    let path = crate::args().nth(1).unwrap();
    let fs = crate::game_fs();
    let Some(text) = fs.read_string(&path) else {
        println!("not found: {path}");
        return;
    };
    for root in kv::parse_document(&text).roots {
        println!("{} {{", root.key);
        walk(&root, 1);
    }
}
