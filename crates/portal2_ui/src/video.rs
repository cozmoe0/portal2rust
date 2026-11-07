//! Bink video playback for the interface: startup videos, menu backgrounds
//! and anything else that draws a movie with the painter.
//!
//! Each video decodes on its own thread a few frames ahead (in the browser,
//! which has one thread, frames decode on the main thread as they come
//! due); the main thread shows the frame for the elapsed real time (frames
//! that are late are skipped) by replacing the image the painter draws. A video's first
//! audio track, when it has one, is decoded up front and played through
//! the mixer when the video starts.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;

use crate::vgui::{ImageRef, Surface};
use portal2_core::vfs::GameFs;

pub struct VideoPlugin;

impl Plugin for VideoPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Videos>().add_systems(PostUpdate, update_videos.before(crate::vgui::VguiPaint));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VideoId(u32);

/// A playing video.
pub struct Video {
    pub path: String,
    pub image: ImageRef,
    pub looping: bool,
    /// Every frame has been shown (never for looping videos).
    pub finished: bool,
    fps: f64,
    frames: u32,
    started: Option<f64>,
    shown: Option<u32>,
    rx: Mutex<Receiver<(u32, Vec<u8>)>>,
    /// Decodes the next frame into the channel when there is no decoder
    /// thread (the browser); false at the end of the video.
    pump: Option<Mutex<Pump>>,
    pending: Option<(u32, Vec<u8>)>,
    stop: Arc<AtomicBool>,
    audio: Option<String>,
    /// Held where it is (an in-world screen nobody can see); its clock
    /// stops too.
    paused: bool,
    paused_at: Option<f64>,
}

impl Drop for Video {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

type Pump = Box<dyn FnMut() -> bool + Send>;

/// Frames decoded on the main thread per update at most; a video further
/// behind than this skips ahead instead of stalling the frame.
#[cfg(target_arch = "wasm32")]
const MAX_INLINE_DECODES: u32 = 3;

impl Video {
    /// The next decoded frame, if one is ready.
    fn next_frame(&mut self, decoded: &mut u32) -> Option<(u32, Vec<u8>)> {
        let rx = self.rx.get_mut().unwrap();
        if let Ok(f) = rx.try_recv() {
            return Some(f);
        }
        let pump = self.pump.as_mut()?.get_mut().unwrap();
        *decoded += 1;
        if !pump() {
            return None;
        }
        rx.try_recv().ok()
    }
}

#[derive(Resource, Default)]
pub struct Videos {
    next: u32,
    playing: Vec<(VideoId, Video)>,
    /// Frames ready for upload, applied once per frame.
    uploads: Vec<(AssetId<Image>, u32, u32, Vec<u8>)>,
    /// Audio tracks to register with the mixer and start.
    audio: Vec<(String, u32, u16, Vec<i16>)>,
}

impl Videos {
    /// Opens `path` (`media/foo.bik`); returns `None` when it cannot be read.
    pub fn open(&mut self, fs: &GameFs, surface: &mut Surface, path: &str, looping: bool) -> Option<VideoId> {
        self.open_with(fs, surface, path, looping, true)
    }

    /// Like `open`; without `sound` the movie's audio track is not played
    /// (in-world screens).
    pub fn open_with(&mut self, fs: &GameFs, surface: &mut Surface, path: &str, looping: bool, sound: bool) -> Option<VideoId> {
        let data = fs.read(path)?;
        let bink = match bink::Bink::open(data) {
            Ok(b) => b,
            Err(e) => {
                warn!("video {path}: {e}");
                return None;
            }
        };
        let (w, h, fps, frames) = (bink.width(), bink.height(), bink.fps(), bink.frame_count());
        let audio = bink.audio_tracks().first().cloned().filter(|_| sound).and_then(|track| {
            let mut dec = bink::AudioDecoder::new(&track).ok()?;
            let mut samples = Vec::new();
            for f in 0..frames {
                if let Ok(Some(packet)) = bink.audio_packet(f, 0) {
                    let _ = dec.decode(packet, &mut samples);
                }
            }
            Some((dec.sample_rate(), dec.channels() as u16, samples))
        });
        let audio_name = audio.as_ref().map(|_| format!("{}#audio", path.to_ascii_lowercase()));
        if let (Some(name), Some((rate, channels, samples))) = (&audio_name, audio) {
            self.audio.push((name.clone(), rate, channels, samples));
        }
        let (tx, rx) = sync_channel(3);
        let stop = Arc::new(AtomicBool::new(false));
        let pump = spawn_decoder(bink, looping, tx, stop.clone()).map(Mutex::new);
        let id = VideoId(self.next);
        self.next += 1;
        let image = ImageRef { id: surface.reserve_image_id(), width: w, height: h, additive: false };
        let video = Video {
            path: path.to_owned(),
            image,
            looping,
            finished: false,
            fps: fps.max(1.0),
            frames,
            started: None,
            shown: None,
            rx: Mutex::new(rx),
            pump,
            pending: None,
            stop,
            audio: audio_name,
            paused: false,
            paused_at: None,
        };
        self.playing.push((id, video));
        Some(id)
    }

    pub fn get(&self, id: VideoId) -> Option<&Video> {
        self.playing.iter().find(|(i, _)| *i == id).map(|(_, v)| v)
    }

    /// Holds a video (or lets it go on): no frames are decoded or shown
    /// while held, and it carries on from the same place.
    pub fn set_paused(&mut self, id: VideoId, paused: bool) {
        if let Some((_, v)) = self.playing.iter_mut().find(|(i, _)| *i == id) {
            v.paused = paused;
        }
    }

    pub fn close(&mut self, id: VideoId) {
        self.playing.retain(|(i, _)| *i != id);
    }

    /// Whether the video has a frame on screen yet.
    pub fn ready(&self, id: VideoId) -> bool {
        self.get(id).is_some_and(|v| v.shown.is_some())
    }
}

/// Decodes one frame into an RGBA buffer, rewinding looping videos at
/// their end; `None` when the video is over or broken.
fn decode_next(bink: &mut bink::Bink, looping: bool) -> Option<(u32, Vec<u8>)> {
    let (w, h) = (bink.width() as usize, bink.height() as usize);
    let mut rewound = false;
    loop {
        match bink.next_frame() {
            Ok(frame) => {
                let index = frame.index();
                let mut rgba = vec![0u8; w * h * 4];
                frame.to_rgba(&mut rgba);
                return Some((index, rgba));
            }
            Err(bink::Error::End) if looping && !rewound => {
                bink.seek(0).ok()?;
                rewound = true;
            }
            Err(_) => return None,
        }
    }
}

/// Starts the video's decoder thread; returns `None` (frames arrive on `tx`).
#[cfg(not(target_arch = "wasm32"))]
fn spawn_decoder(mut bink: bink::Bink, looping: bool, tx: SyncSender<(u32, Vec<u8>)>, stop: Arc<AtomicBool>) -> Option<Pump> {
    std::thread::Builder::new()
        .name("bink".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let Some(frame) = decode_next(&mut bink, looping) else { return };
                if tx.send(frame).is_err() {
                    return;
                }
            }
        })
        .expect("spawn video decoder");
    None
}

/// Without threads, the returned pump decodes a frame each time it is called.
#[cfg(target_arch = "wasm32")]
fn spawn_decoder(mut bink: bink::Bink, looping: bool, tx: SyncSender<(u32, Vec<u8>)>, _stop: Arc<AtomicBool>) -> Option<Pump> {
    Some(Box::new(move || decode_next(&mut bink, looping).is_some_and(|f| tx.try_send(f).is_ok())))
}

fn update_videos(world: &mut World) {
    let now = world.resource::<Time<Real>>().elapsed_secs_f64();
    let audio = std::mem::take(&mut world.resource_mut::<Videos>().audio);
    for (name, rate, channels, samples) in audio {
        portal2_audio::register_pcm(world, &format!("sound/{name}"), rate, channels, samples);
    }
    let mut start_audio = Vec::new();
    {
        let mut videos = world.resource_mut::<Videos>();
        let videos = &mut *videos;
        for (_, v) in videos.playing.iter_mut() {
            if v.finished {
                continue;
            }
            if v.paused {
                if v.started.is_some() && v.paused_at.is_none() {
                    v.paused_at = Some(now);
                }
                continue;
            }
            if let (Some(at), Some(started)) = (v.paused_at.take(), v.started.as_mut()) {
                *started += now - at;
            }
            let mut decoded = 0;
            // Timing starts with the first decoded frame.
            if v.started.is_none() {
                let Some(first) = v.next_frame(&mut decoded) else { continue };
                v.started = Some(now);
                v.pending = Some(first);
                if let Some(a) = &v.audio {
                    start_audio.push(a.clone());
                }
            }
            let elapsed = now - v.started.unwrap_or(now);
            let mut target = (elapsed * v.fps) as u64;
            if !v.looping && target >= v.frames as u64 {
                v.finished = true;
                continue;
            }
            if v.looping {
                target %= v.frames.max(1) as u64;
            }
            let target = target as u32;
            let mut best: Option<(u32, Vec<u8>)> = None;
            loop {
                let next = match v.pending.take() {
                    Some(p) => p,
                    None => {
                        // Decoding on this thread: a video that fell far
                        // behind (a blocking map load) skips ahead.
                        #[cfg(target_arch = "wasm32")]
                        if decoded >= MAX_INLINE_DECODES {
                            v.started = Some(now - v.shown.unwrap_or(0) as f64 / v.fps);
                            break;
                        }
                        match v.next_frame(&mut decoded) {
                            Some(p) => p,
                            None => break,
                        }
                    }
                };
                let due = match v.shown {
                    // Looping restarts at frame 0: a smaller index after the
                    // shown one is the next lap.
                    Some(s) if next.0 < s && v.looping => target < s,
                    _ => next.0 <= target,
                };
                if due {
                    best = Some(next);
                } else {
                    v.pending = Some(next);
                    break;
                }
            }
            if let Some((index, rgba)) = best {
                v.shown = Some(index);
                videos.uploads.push((v.image.id, v.image.width, v.image.height, rgba));
            }
        }
    }
    let uploads = std::mem::take(&mut world.resource_mut::<Videos>().uploads);
    let mut images = world.resource_mut::<Assets<Image>>();
    for (id, w, h, rgba) in uploads {
        let _ = images.insert(id, crate::vgui::surface::rgba_image(w, h, rgba, false));
    }
    for name in start_audio {
        portal2_audio::emit(world, &name, portal2_audio::Emit { soundlevel: Some(0.0), ..default() });
    }
}
