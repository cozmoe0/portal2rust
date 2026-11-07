//! The HTTP(S) server: the page, the WebAssembly package and the asset API.
//!
//! | Request | Answer |
//! | --- | --- |
//! | `GET /` | the page (`web/index.html`) and its `loader.js` |
//! | `GET /pkg/<file>` | the browser build, brotli or gzip compressed when built so |
//! | `GET /api/info` | index version, file count, loose folders, build id (JSON) |
//! | `GET /api/index` | every game file: `path\tsize\tversion` lines (gzip) |
//! | `GET /api/manifest/<phase>` | the files `phase` read before, same lines |
//! | `POST /api/pack[?z=1]` | the files listed in the body, one stream (gzip with `z=1`) |
//! | `GET /api/file/<key>` | one file (the browser's fallback for a file it lacks) |
//! | `POST /api/learn/<phase>` | files a browser read during `phase` |
//! | `POST /api/next/<map>` | the map a browser loaded after `map` |
//!
//! A manifest's `x-p2r-next` header names the map usually played next.
//!
//! A pack is a sequence of records: key length (u32 LE), key (UTF-8), data
//! length (u32 LE, `u32::MAX` for a missing file), data.

use std::convert::Infallible;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Body, Frame, Incoming};
use hyper::header::{self, HeaderValue};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::Options;
use crate::assets::{GameAssets, phase_name};

type ResBody = BoxBody<Bytes, Infallible>;

/// Largest request body accepted (file lists).
const MAX_BODY: usize = 8 << 20;
/// Size of the chunks a pack is sent in.
const CHUNK: usize = 256 << 10;

struct State {
    assets: GameAssets,
    pkg: PathBuf,
    web: WebFiles,
}

/// The page's own files: read from the source tree when it is there (so
/// they can be edited without rebuilding), otherwise the copies built in.
struct WebFiles {
    dir: Option<PathBuf>,
}

impl WebFiles {
    fn get(&self, name: &str) -> Option<Vec<u8>> {
        if let Some(dir) = &self.dir
            && let Ok(data) = std::fs::read(dir.join(name))
        {
            return Some(data);
        }
        match name {
            "index.html" => Some(include_bytes!("../web/index.html").to_vec()),
            "loader.js" => Some(include_bytes!("../web/loader.js").to_vec()),
            _ => None,
        }
    }
}

pub fn serve(opts: &Options) -> Result<(), String> {
    let data_dir = opts.data_dir();
    let assets = GameAssets::mount(opts.game.as_deref(), &data_dir)?;
    let pkg = opts.pkg_dir();
    if !pkg.join("portal2_bg.wasm").is_file() {
        eprintln!("p2web: warning: no browser build in {} yet (run `p2web build`)", pkg.display());
    }
    let source_web = Path::new(env!("CARGO_MANIFEST_DIR")).join("web");
    let state = Arc::new(State { assets, pkg, web: WebFiles { dir: source_web.is_dir().then_some(source_web) } });
    let tls = if opts.http { None } else { Some(crate::tls::acceptor(opts, &data_dir)?) };
    let addr: SocketAddr = format!("{}:{}", opts.bind, opts.port).parse().map_err(|e| format!("bad address: {e}"))?;
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async move {
        let listener = TcpListener::bind(addr).await.map_err(|e| format!("cannot listen on {addr}: {e}"))?;
        print_urls(opts, tls.is_some());
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("p2web: accept: {e}");
                    continue;
                }
            };
            let _ = stream.set_nodelay(true);
            let state = state.clone();
            let tls = tls.clone();
            tokio::spawn(async move {
                let service = service_fn(move |req| {
                    let state = state.clone();
                    async move { Ok::<_, Infallible>(handle(state, req, peer).await) }
                });
                let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
                match tls {
                    Some(acceptor) => {
                        // Someone typed http:// for an https:// server: send
                        // them to the right address.
                        let mut first = [0u8; 1];
                        if stream.peek(&mut first).await.is_ok() && first[0] != 0x16 {
                            let redirect = service_fn(|req: Request<Incoming>| async move { Ok::<_, Infallible>(to_https(&req)) });
                            let _ = builder.serve_connection(TokioIo::new(stream), redirect).await;
                            return;
                        }
                        match acceptor.accept(stream).await {
                            Ok(tls_stream) => {
                                let _ = builder.serve_connection(TokioIo::new(tls_stream), service).await;
                            }
                            Err(e) => eprintln!("p2web: TLS with {peer}: {e}"),
                        }
                    }
                    None => {
                        let _ = builder.serve_connection(TokioIo::new(stream), service).await;
                    }
                }
            });
        }
    })
}

fn print_urls(opts: &Options, https: bool) {
    let scheme = if https { "https" } else { "http" };
    println!("p2web: serving portal2rust on port {} ({})", opts.port, if https { "HTTPS" } else { "plain HTTP" });
    println!("  this machine:   {scheme}://localhost:{}/", opts.port);
    if opts.bind == "0.0.0.0" || opts.bind == "::" {
        for ip in crate::tls::local_ipv4() {
            println!("  on the network: {scheme}://{ip}:{}/", opts.port);
        }
    }
    if https {
        println!("  The certificate is self-signed: the browser warns once per machine (Advanced > Proceed).");
    } else {
        println!("  Plain HTTP: browsers only allow WebGPU on http://localhost; other machines need HTTPS.");
    }
    println!("  Anyone who can reach this port can download the game's files; serve on networks you trust.");
}

fn to_https(req: &Request<Incoming>) -> Response<ResBody> {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("localhost");
    let target = format!("https://{host}{}", req.uri().path_and_query().map_or("/", |p| p.as_str()));
    let mut res = text(StatusCode::MOVED_PERMANENTLY, "use https");
    if let Ok(v) = HeaderValue::from_str(&target) {
        res.headers_mut().insert(header::LOCATION, v);
    }
    res
}

fn full(data: impl Into<Bytes>) -> ResBody {
    Full::new(data.into()).boxed()
}

fn text(status: StatusCode, body: &str) -> Response<ResBody> {
    let mut res = Response::new(full(body.to_owned()));
    *res.status_mut() = status;
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    res
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

/// Undoes `encodeURIComponent`.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

async fn body_text(req: Request<Incoming>) -> Option<String> {
    let collected = Limited::new(req.into_body(), MAX_BODY).collect().await.ok()?;
    String::from_utf8(collected.to_bytes().to_vec()).ok()
}

async fn handle(state: Arc<State>, req: Request<Incoming>, peer: SocketAddr) -> Response<ResBody> {
    let started = Instant::now();
    let path = req.uri().path().to_owned();
    let method = req.method().clone();
    let gzip_ok = req.headers().get(header::ACCEPT_ENCODING).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("gzip"));
    let br_ok = req.headers().get(header::ACCEPT_ENCODING).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("br"));
    let mut res = match (&method, path.as_str()) {
        (&Method::GET, "/" | "/index.html") => page(&state, "index.html"),
        (&Method::GET, "/loader.js") => page(&state, "loader.js"),
        (&Method::GET, "/favicon.ico") => text(StatusCode::NO_CONTENT, ""),
        (&Method::GET, "/api/info") => info(&state),
        (&Method::GET, "/api/index") => {
            let mut res = Response::new(full(state.assets.index_gz.clone()));
            let h = res.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
            h.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
            res
        }
        (&Method::GET, p) if p.starts_with("/api/manifest/") => match phase_name(&p["/api/manifest/".len()..]) {
            Some(phase) => {
                let mut res = text(StatusCode::OK, &state.assets.manifest(&phase));
                if let Some(next) = state.assets.next_of(&phase)
                    && let Ok(v) = HeaderValue::from_str(&next)
                {
                    res.headers_mut().insert("x-p2r-next", v);
                }
                res
            }
            None => text(StatusCode::BAD_REQUEST, "bad phase"),
        },
        (&Method::GET, p) if p.starts_with("/api/file/") => file(&state, &p["/api/file/".len()..], peer).await,
        (&Method::GET, p) if p.starts_with("/pkg/") => pkg(&state, &p["/pkg/".len()..], br_ok, gzip_ok),
        (&Method::POST, "/api/pack") => {
            let z = req.uri().query().is_some_and(|q| q.split('&').any(|kv| kv == "z=1"));
            match body_text(req).await {
                Some(body) => pack(state.clone(), body, z),
                None => text(StatusCode::PAYLOAD_TOO_LARGE, "list too large"),
            }
        }
        (&Method::POST, p) if p.starts_with("/api/next/") => {
            let from = phase_name(&p["/api/next/".len()..]);
            match (from, body_text(req).await.as_deref().and_then(phase_name)) {
                (Some(from), Some(to)) if from != to => {
                    state.assets.learn_next(&from, &to);
                    text(StatusCode::NO_CONTENT, "")
                }
                _ => text(StatusCode::BAD_REQUEST, "bad request"),
            }
        }
        (&Method::POST, p) if p.starts_with("/api/learn/") => {
            let phase = phase_name(&p["/api/learn/".len()..]);
            match (phase, body_text(req).await) {
                (Some(phase), Some(body)) => {
                    let added = state.assets.learn(&phase, &body);
                    if added > 0 {
                        println!("p2web: {peer} taught {added} new files for {phase}");
                    }
                    text(StatusCode::NO_CONTENT, "")
                }
                _ => text(StatusCode::BAD_REQUEST, "bad request"),
            }
        }
        _ => text(StatusCode::NOT_FOUND, "not found"),
    };
    // Cross-origin isolation gives the page precise timers (frame timing).
    let h = res.headers_mut();
    h.insert("cross-origin-opener-policy", HeaderValue::from_static("same-origin"));
    h.insert("cross-origin-embedder-policy", HeaderValue::from_static("require-corp"));
    if path == "/" || path.starts_with("/pkg/") {
        println!("p2web: {peer} {method} {path} {} ({:.0} ms)", res.status().as_u16(), started.elapsed().as_secs_f64() * 1000.0);
    }
    res
}

fn page(state: &State, name: &str) -> Response<ResBody> {
    let Some(data) = state.web.get(name) else { return text(StatusCode::NOT_FOUND, "not found") };
    let mut res = Response::new(full(data));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(name)));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    res
}

fn info(state: &State) -> Response<ResBody> {
    let build = std::fs::read_to_string(state.pkg.join("build.txt")).unwrap_or_default();
    let loose: Vec<String> = state.assets.loose.iter().map(|l| json_string(l)).collect();
    let json = format!(
        "{{\"version\":{},\"files\":{},\"loose\":[{}],\"build\":{}}}",
        json_string(&state.assets.version),
        state.assets.index.len(),
        loose.join(","),
        json_string(build.trim())
    );
    let mut res = Response::new(full(json));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// One file of the browser build, preferring the precompressed copies.
fn pkg(state: &State, rel: &str, br_ok: bool, gzip_ok: bool) -> Response<ResBody> {
    if rel.contains("..") || rel.contains('\\') || rel.is_empty() {
        return text(StatusCode::BAD_REQUEST, "bad path");
    }
    let path = state.pkg.join(rel);
    let variants: [(bool, &str, Option<&'static str>); 3] = [(br_ok, ".br", Some("br")), (gzip_ok, ".gz", Some("gzip")), (true, "", None)];
    for (ok, suffix, encoding) in variants {
        if !ok {
            continue;
        }
        let mut file = path.clone().into_os_string();
        file.push(suffix);
        let Ok(data) = std::fs::read(&file) else { continue };
        let size = std::fs::metadata(&path).map_or(0, |m| m.len());
        let mut res = Response::new(full(data));
        let h = res.headers_mut();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(rel)));
        // The uncompressed size, for the page's progress bar.
        h.insert("x-p2r-size", HeaderValue::from(size));
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        if let Some(enc) = encoding {
            h.insert(header::CONTENT_ENCODING, HeaderValue::from_static(enc));
        }
        return res;
    }
    text(StatusCode::NOT_FOUND, "not built")
}

async fn file(state: &Arc<State>, raw: &str, peer: SocketAddr) -> Response<ResBody> {
    let Some(key) = percent_decode(raw) else { return text(StatusCode::BAD_REQUEST, "bad key") };
    let Some((_, version)) = state.assets.stat(&key) else { return text(StatusCode::NOT_FOUND, "not found") };
    let st = state.clone();
    let k = key.clone();
    let data = tokio::task::spawn_blocking(move || st.assets.read(&k)).await.ok().flatten();
    let Some(data) = data else { return text(StatusCode::NOT_FOUND, "not found") };
    println!("p2web: {peer} fetched {key} on its own ({} KiB)", data.len() / 1024);
    let mut res = Response::new(full(data));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    if let Ok(v) = HeaderValue::from_str(&format!("{version:x}")) {
        h.insert("x-p2r-version", v);
    }
    res
}

/// A response body fed from a channel.
struct ChannelBody(mpsc::Receiver<Bytes>);

impl Body for ChannelBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        self.0.poll_recv(cx).map(|chunk| chunk.map(|b| Ok(Frame::data(b))))
    }
}

/// Collects written bytes into chunks sent down a channel; fails once the
/// client has gone.
struct ChannelWriter {
    tx: mpsc::Sender<Bytes>,
    buf: Vec<u8>,
}

impl ChannelWriter {
    fn send(&mut self) -> std::io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let chunk = Bytes::from(std::mem::replace(&mut self.buf, Vec::with_capacity(CHUNK)));
        self.tx.blocking_send(chunk).map_err(|_| std::io::ErrorKind::BrokenPipe.into())
    }
}

impl Write for ChannelWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        if self.buf.len() >= CHUNK {
            self.send()?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.send()
    }
}

fn write_pack(state: &State, keys: &[&str], out: &mut dyn Write) -> std::io::Result<()> {
    for key in keys {
        out.write_all(&(key.len() as u32).to_le_bytes())?;
        out.write_all(key.as_bytes())?;
        match state.assets.read(key) {
            Some(data) => {
                out.write_all(&(data.len() as u32).to_le_bytes())?;
                out.write_all(&data)?;
            }
            None => out.write_all(&u32::MAX.to_le_bytes())?,
        }
    }
    out.flush()
}

/// Streams the listed files, read on a blocking thread.
fn pack(state: Arc<State>, body: String, z: bool) -> Response<ResBody> {
    let (tx, rx) = mpsc::channel(8);
    tokio::task::spawn_blocking(move || {
        let keys: Vec<&str> = body.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        let writer = ChannelWriter { tx, buf: Vec::with_capacity(CHUNK) };
        let _ = if z {
            let mut gz = flate2::write::GzEncoder::new(writer, flate2::Compression::fast());
            write_pack(&state, &keys, &mut gz).and_then(|()| gz.finish().and_then(|mut w| w.flush()))
        } else {
            let mut w = writer;
            write_pack(&state, &keys, &mut w)
        };
    });
    let mut res = Response::new(ChannelBody(rx).boxed());
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if z {
        h.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    res
}
