//! Decoder for RAD Game Tools' Bink 1 video files (`.bik`).
//!
//! Pure Rust, no game data: it reads the container, decodes video frames to
//! 8-bit YUV 4:2:0 planes (plus alpha when present) and audio tracks to
//! 16-bit PCM. Portal 2 uses Bink for its menu backgrounds and in-world
//! screens. Format notes live in `docs/re/bink.md`.
//!
//! ```no_run
//! let mut video = bink::Bink::open(std::fs::read("menu_act01.bik")?)?;
//! let mut rgba = vec![0; video.width() as usize * video.height() as usize * 4];
//! while video.position() < video.frame_count() {
//!     video.next_frame()?.to_rgba(&mut rgba);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod audio;
mod bits;
mod container;
mod frame;
mod tables;
mod video;

pub use audio::AudioDecoder;
pub use container::{AudioTrack, Header, audio_flags, flags};
pub use frame::{Frame, Plane, yuv_to_rgb};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a Bink file")]
    NotBink,
    #[error("unsupported Bink revision {0:?}")]
    Revision(char),
    #[error("corrupt header: {0}")]
    Header(String),
    #[error("unexpected end of data")]
    Truncated,
    #[error("frame {frame}: {msg}")]
    Video { frame: u32, msg: &'static str },
    #[error("audio: {0}")]
    Audio(&'static str),
    #[error("audio track {0} does not exist")]
    NoTrack(usize),
    #[error("frame {0} is past the end of the video")]
    FrameRange(u32),
    #[error("end of video")]
    End,
}

pub type Result<T> = std::result::Result<T, Error>;

/// An open Bink file with its video decoding state.
pub struct Bink {
    data: Vec<u8>,
    header: Header,
    index: Vec<container::IndexEntry>,
    decoder: video::VideoDecoder,
    /// The last decoded frame is `frames[0]`; `frames[1]` is scratch space
    /// holding the frame before it.
    frames: [Frame; 2],
    decoded: bool,
    next: u32,
}

impl Bink {
    /// Parses the header and frame index of a whole file held in memory.
    pub fn open(data: Vec<u8>) -> Result<Self> {
        let (header, index) = container::parse(&data)?;
        let decoder = video::VideoDecoder::new(header.revision, header.has_alpha(), header.is_grayscale());
        let blank = || {
            let mut f = Frame::new(header.width, header.height, header.has_alpha());
            if header.is_grayscale() {
                f.u.data.fill(128);
                f.v.data.fill(128);
            }
            f
        };
        let frames = [blank(), blank()];
        Ok(Self { data, header, index, decoder, frames, decoded: false, next: 0 })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn width(&self) -> u32 {
        self.header.width
    }

    pub fn height(&self) -> u32 {
        self.header.height
    }

    /// Frames per second.
    pub fn fps(&self) -> f64 {
        self.header.fps_num as f64 / self.header.fps_den as f64
    }

    pub fn frame_count(&self) -> u32 {
        self.header.frame_count
    }

    pub fn is_keyframe(&self, frame: u32) -> bool {
        frame == 0 || self.index.get(frame as usize).is_some_and(|e| e.keyframe)
    }

    pub fn audio_tracks(&self) -> &[AudioTrack] {
        &self.header.audio
    }

    /// Index of the frame the next call to [`Bink::next_frame`] decodes.
    pub fn position(&self) -> u32 {
        self.next
    }

    /// The most recently decoded frame.
    pub fn current_frame(&self) -> Option<&Frame> {
        self.decoded.then_some(&self.frames[0])
    }

    fn frame_bytes(&self, frame: u32) -> Result<&[u8]> {
        frame_bytes(&self.data, &self.index, frame)
    }

    /// The audio packet of `track` in `frame`, if it has one. Feed packets
    /// in frame order to an [`AudioDecoder`].
    pub fn audio_packet(&self, frame: u32, track: usize) -> Result<Option<&[u8]>> {
        if track >= self.header.audio.len() {
            return Err(Error::NoTrack(track));
        }
        let (audio, _) = container::split_frame(self.frame_bytes(frame)?, self.header.audio.len())?;
        Ok(Some(audio[track]).filter(|p| !p.is_empty()))
    }

    /// Decodes the next frame.
    pub fn next_frame(&mut self) -> Result<&Frame> {
        let n = self.next;
        if n >= self.header.frame_count {
            return Err(Error::End);
        }
        let keyframe = self.is_keyframe(n);
        let bytes = frame_bytes(&self.data, &self.index, n)?;
        let (_, packet) = container::split_frame(bytes, self.header.audio.len())?;
        // `frames[0]` holds the previous frame; decode into the older buffer.
        let [prev, cur] = &mut self.frames;
        self.decoder.decode(packet, cur, prev).map_err(|msg| Error::Video { frame: n, msg })?;
        self.frames.swap(0, 1);
        self.frames[0].index = n;
        self.frames[0].keyframe = keyframe;
        self.decoded = true;
        self.next = n + 1;
        Ok(&self.frames[0])
    }

    /// Positions the decoder so that the next call to [`Bink::next_frame`]
    /// returns `frame`, decoding forward from the closest keyframe before it.
    pub fn seek(&mut self, frame: u32) -> Result<()> {
        if frame >= self.header.frame_count {
            return Err(Error::FrameRange(frame));
        }
        let key = (0..=frame).rev().find(|&f| self.is_keyframe(f)).unwrap_or(0);
        // Continue from the current position when that is already on the way.
        if !(key <= self.next && self.next <= frame) {
            self.next = key;
        }
        while self.next < frame {
            self.next_frame()?;
        }
        Ok(())
    }
}

fn frame_bytes<'a>(data: &'a [u8], index: &[container::IndexEntry], frame: u32) -> Result<&'a [u8]> {
    let i = frame as usize;
    match (index.get(i), index.get(i + 1)) {
        (Some(a), Some(b)) => Ok(&data[a.offset as usize..b.offset as usize]),
        _ => Err(Error::FrameRange(frame)),
    }
}
