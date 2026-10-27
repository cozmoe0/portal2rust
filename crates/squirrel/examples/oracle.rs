//! Runs one-line snippets (one per line, `#` comments) and prints each
//! output followed by a separator, for `re/tools/vscript_oracle.py`.

fn capture(host: &mut dyn std::any::Any, text: &str) {
    if let Some(out) = host.downcast_mut::<String>() {
        out.push_str(text);
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("cases file");
    let text = std::fs::read_to_string(path).expect("readable cases");
    for line in text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
        let mut vm = squirrel::Vm::new();
        vm.print = capture;
        let mut out = String::new();
        let root = vm.root_value();
        if let Err(e) = vm.run_script(&mut out, line.as_bytes(), "oracle", &root) {
            let _ = e;
            out = "<no output>".into();
        }
        print!("{out}\n<<<end>>>\n");
    }
}
