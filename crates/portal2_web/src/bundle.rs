//! `p2web build`: the game compiled to WebAssembly, its JavaScript bindings
//! generated, and both compressed ahead of time so the server sends them
//! small without compressing on every request.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::Options;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// The version of a package in the workspace's `Cargo.lock`.
fn locked_version(lock: &str, name: &str) -> Option<String> {
    let header = format!("name = \"{name}\"\n");
    let at = lock.replace("\r\n", "\n");
    let rest = &at[at.find(&header)? + header.len()..];
    let version = rest.strip_prefix("version = \"")?;
    Some(version[..version.find('"')?].to_owned())
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

/// What the patch changes, and a marker for copies already patched.
const WGPU_PATCH: &str = "p2r-wgpu-patch-3";

/// Textures and buffers destroyed a frame after their last handle drops.
const GC_MODULE: &str = r#"
/// portal2rust: a browser frees a texture's or buffer's memory only when it
/// is destroyed or its JavaScript object is garbage collected, and this
/// backend left dropped ones to the collector, which knows nothing of GPU
/// memory: render targets reallocated on a resize piled up until the GPU
/// paged. Each texture and buffer gets a guard that its views and the bind
/// groups using it share, as native wgpu's own references work; when the
/// last one goes, the resource is destroyed at the start of the next frame,
/// by which time every command using it has been submitted.
mod p2r_gc {
    use alloc::{rc::Rc, vec::Vec};
    use core::cell::RefCell;

    use super::webgpu_sys;

    pub enum Kind {
        Texture(webgpu_sys::GpuTexture),
        Buffer(webgpu_sys::GpuBuffer),
    }

    pub struct Guard(Option<Kind>);

    impl core::fmt::Debug for Guard {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.write_str("Guard")
        }
    }

    struct State {
        frame: u64,
        pending: Vec<(u64, Kind)>,
    }

    struct Shared(RefCell<State>);

    // SAFETY: the WebGPU backend runs on the page's one thread.
    unsafe impl Sync for Shared {}

    static STATE: Shared = Shared(RefCell::new(State { frame: 0, pending: Vec::new() }));

    pub fn guard(kind: Kind) -> Rc<Guard> {
        Rc::new(Guard(Some(kind)))
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            // (wgpu is a 2021-edition crate: no `let` chains.)
            let Some(kind) = self.0.take() else { return };
            if let Ok(mut state) = STATE.0.try_borrow_mut() {
                let frame = state.frame;
                state.pending.push((frame, kind));
            }
        }
    }

    /// A new frame: what was dropped in an earlier one goes.
    pub fn next_frame() {
        let due: Vec<Kind> = {
            let Ok(mut state) = STATE.0.try_borrow_mut() else { return };
            state.frame += 1;
            let frame = state.frame;
            let (due, keep): (Vec<_>, Vec<_>) = state.pending.drain(..).partition(|(f, _)| *f < frame);
            state.pending = keep;
            due.into_iter().map(|(_, k)| k).collect()
        };
        for kind in due {
            match kind {
                Kind::Texture(t) => t.destroy(),
                Kind::Buffer(b) => b.destroy(),
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct WebBindGroup {"#;

/// wgpu's WebGPU backend reads a device's features and limits back from the
/// browser on every query (about fifty calls into JavaScript), and Bevy asks
/// for the limits on every draw: a sixth of a frame's time in busy views.
/// Both never change for a device, so the browser build uses a copy of the
/// locked wgpu release that keeps the first answer. The same copy destroys
/// textures and buffers once nothing refers to them (see `GC_MODULE`). The
/// copy lives in the data folder and only this build points Cargo at it.
/// `None` (and a warning) when the source is not where Cargo keeps it or no
/// longer looks as expected.
fn patched_wgpu(opts: &Options) -> Option<PathBuf> {
    let lock = std::fs::read_to_string(workspace_root().join("Cargo.lock")).ok()?;
    let version = locked_version(&lock, "wgpu")?;
    let dest = opts.data_dir().join("patched").join(format!("wgpu-{version}"));
    if std::fs::read_to_string(dest.join(WGPU_PATCH)).is_ok() {
        return Some(dest);
    }
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(|h| PathBuf::from(h).join(".cargo")))?;
    let source = std::fs::read_dir(cargo_home.join("registry").join("src"))
        .ok()?
        .flatten()
        .map(|d| d.path().join(format!("wgpu-{version}")))
        .find(|p| p.join("src/backend/webgpu.rs").is_file());
    let Some(source) = source else {
        eprintln!("p2web: warning: wgpu {version} source not found; building without the WebGPU limits cache");
        return None;
    };
    let _ = std::fs::remove_dir_all(&dest);
    if let Err(e) = copy_dir(&source, &dest) {
        eprintln!("p2web: warning: cannot copy wgpu: {e}");
        return None;
    }
    let file = dest.join("src/backend/webgpu.rs");
    let text = std::fs::read_to_string(&file).ok()?;
    let edits = [
        (
            "    error_scope_count: Rc<Cell<u32>>,\n}",
            "    error_scope_count: Rc<Cell<u32>>,\n    /// portal2rust: features and limits, read from the browser once.\n    p2r_caps: core::cell::OnceCell<(crate::Features, crate::Limits)>,\n}",
        ),
        (
            "                    error_scope_count: Rc::new(Cell::new(0)),\n",
            "                    error_scope_count: Rc::new(Cell::new(0)),\n                    p2r_caps: core::cell::OnceCell::new(),\n",
        ),
        (
            "impl dispatch::DeviceInterface for WebDevice {\n    fn features(&self) -> crate::Features {\n        map_wgt_features(self.inner.features())\n    }\n\n    fn limits(&self) -> crate::Limits {\n        map_wgt_limits(self.inner.limits())\n    }",
            "impl WebDevice {\n    fn p2r_caps(&self) -> &(crate::Features, crate::Limits) {\n        self.p2r_caps.get_or_init(|| (map_wgt_features(self.inner.features()), map_wgt_limits(self.inner.limits())))\n    }\n}\n\nimpl dispatch::DeviceInterface for WebDevice {\n    fn features(&self) -> crate::Features {\n        self.p2r_caps().0\n    }\n\n    fn limits(&self) -> crate::Limits {\n        self.p2r_caps().1.clone()\n    }",
        ),
        // Destroying textures and buffers no longer referred to.
        ("#[derive(Debug, Clone)]\npub struct WebBindGroup {", GC_MODULE),
        (
            "    pub(crate) inner: webgpu_sys::GpuBindGroup,\n    /// Unique identifier for this BindGroup.",
            "    pub(crate) inner: webgpu_sys::GpuBindGroup,\n    /// portal2rust: keeps what the group refers to alive.\n    #[allow(dead_code)]\n    p2r: Vec<Rc<p2r_gc::Guard>>,\n    /// Unique identifier for this BindGroup.",
        ),
        (
            "    pub(crate) inner: webgpu_sys::GpuTextureView,\n    /// Unique identifier for this TextureView.",
            "    pub(crate) inner: webgpu_sys::GpuTextureView,\n    p2r: Option<Rc<p2r_gc::Guard>>,\n    /// Unique identifier for this TextureView.",
        ),
        (
            "    mapping: Rc<RefCell<WebBufferMapState>>,\n    /// Unique identifier for this Buffer.",
            "    mapping: Rc<RefCell<WebBufferMapState>>,\n    p2r: Option<Rc<p2r_gc::Guard>>,\n    /// Unique identifier for this Buffer.",
        ),
        (
            "    fn new(inner: webgpu_sys::GpuBuffer, desc: &crate::BufferDescriptor<'_>) -> Self {\n        Self {\n            inner,",
            "    fn new(inner: webgpu_sys::GpuBuffer, desc: &crate::BufferDescriptor<'_>) -> Self {\n        Self {\n            p2r: Some(p2r_gc::guard(p2r_gc::Kind::Buffer(inner.clone()))),\n            inner,",
        ),
        (
            "    drop_guard: Option<Rc<DropGuard>>,\n    /// Unique identifier for this Texture.",
            "    drop_guard: Option<Rc<DropGuard>>,\n    p2r: Option<Rc<p2r_gc::Guard>>,\n    /// Unique identifier for this Texture.",
        ),
        (
            "        let texture = self.inner.create_texture(&mapped_desc).unwrap();\n        WebTexture {\n            inner: texture,",
            "        let texture = self.inner.create_texture(&mapped_desc).unwrap();\n        WebTexture {\n            p2r: Some(p2r_gc::guard(p2r_gc::Kind::Texture(texture.clone()))),\n            inner: texture,",
        ),
        (
            "        let web_surface_texture = WebTexture {\n            inner: surface_texture,",
            "        let web_surface_texture = WebTexture {\n            p2r: None,\n            inner: surface_texture,",
        ),
        (
            "            drop_guard: Some(Rc::new(DropGuard::new(drop_callback))),\n",
            "            drop_guard: Some(Rc::new(DropGuard::new(drop_callback))),\n            p2r: None,\n",
        ),
        (
            "        WebTextureView {\n            inner: view,\n",
            "        WebTextureView {\n            p2r: self.p2r.clone(),\n            inner: view,\n",
        ),
        (
            "        let mapped_entries = desc\n            .entries\n            .iter()\n            .map(|binding| match binding.resource {",
            "        let mut p2r_guards = Vec::new();\n        let mapped_entries = desc\n            .entries\n            .iter()\n            .map(|binding| match binding.resource {",
        ),
        (
            "                    let buffer = buffer.inner.as_webgpu();\n                    let mapped_buffer_binding",
            "                    let buffer = buffer.inner.as_webgpu();\n                    p2r_guards.extend(buffer.p2r.clone());\n                    let mapped_buffer_binding",
        ),
        (
            "                    let texture_view = &texture_view.inner.as_webgpu().inner;\n",
            "                    let texture_view = texture_view.inner.as_webgpu();\n                    p2r_guards.extend(texture_view.p2r.clone());\n                    let texture_view = &texture_view.inner;\n",
        ),
        (
            "        WebBindGroup {\n            inner: bind_group,\n",
            "        WebBindGroup {\n            p2r: p2r_guards,\n            inner: bind_group,\n",
        ),
        (
            "        let detail = WebSurfaceOutputDetail {\n",
            "        p2r_gc::next_frame();\n        let detail = WebSurfaceOutputDetail {\n",
        ),
    ];
    let mut patched = text.replace("\r\n", "\n");
    for (from, to) in edits {
        if patched.matches(from).count() != 1 {
            eprintln!("p2web: warning: wgpu {version} changed shape; building without the WebGPU patch");
            let _ = std::fs::remove_dir_all(&dest);
            return None;
        }
        patched = patched.replace(from, to);
    }
    std::fs::write(&file, patched).ok()?;
    std::fs::write(dest.join(WGPU_PATCH), "features and limits cached per device; unused textures and buffers destroyed\n").ok()?;
    Some(dest)
}

pub fn build(opts: &Options) -> Result<(), String> {
    let started = Instant::now();
    let profile = if opts.dev { "web-dev" } else { "web" };
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let target = crate::workspace_target();
    println!("p2web: building the game for the browser (profile {profile}){}", if opts.dev { "" } else { "; a full build takes a while" });
    let mut cmd = Command::new(&cargo);
    cmd.current_dir(workspace_root())
        .args(["build", "-p", "portal2", "--bin", "portal2", "--target", "wasm32-unknown-unknown", "--profile", profile])
        .env("CARGO_TARGET_DIR", &target);
    // The patched wgpu changes Cargo.lock for this build only.
    let lock_path = workspace_root().join("Cargo.lock");
    let lock = std::fs::read(&lock_path).ok();
    if let Some(wgpu) = patched_wgpu(opts) {
        let dir = wgpu.to_string_lossy().replace('\\', "/");
        cmd.arg("--config").arg(format!("patch.crates-io.wgpu.path=\"{dir}\""));
    }
    let status = cmd.status().map_err(|e| format!("cannot run cargo: {e}"));
    if let Some(lock) = lock {
        let _ = std::fs::write(&lock_path, lock);
    }
    let status = status?;
    if !status.success() {
        return Err("the browser build failed (is the target installed? `rustup target add wasm32-unknown-unknown`)".into());
    }
    let wasm = target.join("wasm32-unknown-unknown").join(profile).join("portal2.wasm");
    let out = opts.pkg_dir();
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    // Old outputs (and their compressed copies) must not outlive this build.
    if let Ok(dir) = std::fs::read_dir(&out) {
        for entry in dir.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    println!("p2web: generating the JavaScript bindings");
    wasm_bindgen_cli_support::Bindgen::new()
        .input_path(&wasm)
        .out_name("portal2")
        .web(true)
        .map_err(|e| e.to_string())?
        .typescript(false)
        .generate(&out)
        .map_err(|e| format!("wasm-bindgen: {e}"))?;
    let bg = out.join("portal2_bg.wasm");
    if opts.opt {
        optimize(&bg);
    }
    let quality = if opts.dev { 5 } else { 9 };
    let mut files = Vec::new();
    collect(&out, &mut files);
    for file in files.iter().filter(|f| f.extension().is_some_and(|e| e == "wasm" || e == "js")) {
        compress(file, quality)?;
    }
    let data = std::fs::read(&bg).map_err(|e| e.to_string())?;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in &data {
        hash = (hash ^ *b as u64).wrapping_mul(0x100_0000_01b3);
    }
    std::fs::write(out.join("build.txt"), format!("{hash:016x}")).map_err(|e| e.to_string())?;
    let br = std::fs::metadata(out.join("portal2_bg.wasm.br")).map_or(0, |m| m.len());
    println!(
        "p2web: browser build ready in {} ({} MiB, {} MiB compressed) after {:.0}s",
        out.display(),
        data.len() >> 20,
        br >> 20,
        started.elapsed().as_secs_f32()
    );
    Ok(())
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// binaryen's `wasm-opt -O3`, when it is installed.
fn optimize(wasm: &Path) {
    let tmp = wasm.with_extension("opt.wasm");
    println!("p2web: running wasm-opt -O3");
    match Command::new("wasm-opt").arg("-O3").arg(wasm).arg("-o").arg(&tmp).status() {
        Ok(s) if s.success() => {
            let _ = std::fs::rename(&tmp, wasm);
        }
        Ok(s) => eprintln!("p2web: wasm-opt failed ({s}); keeping the unoptimised module"),
        Err(_) => eprintln!("p2web: wasm-opt not found (install binaryen); skipped"),
    }
}

/// Writes `<file>.br` and `<file>.gz` beside the file.
fn compress(file: &Path, quality: u32) -> Result<(), String> {
    let data = std::fs::read(file).map_err(|e| e.to_string())?;
    let with = |ext: &str| {
        let mut p = file.as_os_str().to_owned();
        p.push(ext);
        PathBuf::from(p)
    };
    let mut br = Vec::with_capacity(data.len() / 3);
    {
        let mut w = brotli::CompressorWriter::new(&mut br, 1 << 16, quality, 22);
        w.write_all(&data).map_err(|e| e.to_string())?;
    }
    std::fs::write(with(".br"), br).map_err(|e| e.to_string())?;
    let mut gz = flate2::write::GzEncoder::new(Vec::with_capacity(data.len() / 3), flate2::Compression::best());
    gz.write_all(&data).map_err(|e| e.to_string())?;
    std::fs::write(with(".gz"), gz.finish().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(())
}
