//! Bink 1 container: file header, audio track table and frame index.
//!
//! Layout (all little-endian):
//!
//! | offset | size | field |
//! | --- | --- | --- |
//! | 0 | 4 | `"BIK"` + revision letter |
//! | 4 | 4 | file size minus 8 |
//! | 8 | 4 | frame count |
//! | 12 | 4 | largest frame size in bytes |
//! | 16 | 4 | frame index length (at least the frame count) |
//! | 20 | 8 | width, height |
//! | 28 | 8 | frame rate numerator, denominator |
//! | 36 | 4 | video flags ([`flags`]) |
//! | 40 | 4 | audio track count |
//!
//! followed by three per-track arrays (maximum decoded size; sample rate and
//! audio flags; track id) and `index length + 1` frame offsets. Bit 0 of an
//! offset marks a keyframe. Each frame holds one size-prefixed packet per
//! audio track, then the video packet up to the next frame's offset.

use crate::{Error, Result};

/// Video flag bits from the file header.
pub mod flags {
    /// The file carries an alpha plane.
    pub const ALPHA: u32 = 0x0010_0000;
    /// Only the luma plane is coded.
    pub const GRAYSCALE: u32 = 0x0002_0000;
    /// Display-time scaling mode (bits 28..=30): 1 or 2 double the height,
    /// 3 doubles the width, 4 or 5 double both. Frames are decoded unscaled.
    pub const SCALING_MASK: u32 = 0x7000_0000;
}

/// Audio flag bits of a track (the high half of its rate/flags word).
pub mod audio_flags {
    /// Bink Audio with the FFT based transform.
    pub const FFT: u16 = 0x8000;
    /// Samples are 16 bit (8 bit otherwise).
    pub const BITS16: u16 = 0x4000;
    pub const STEREO: u16 = 0x2000;
    /// Bink Audio with the DCT based transform.
    pub const DCT: u16 = 0x1000;
}

/// Size of the fixed part of the header.
const FIXED_HEADER: usize = 44;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioTrack {
    pub id: u32,
    pub sample_rate: u32,
    /// Audio flag bits ([`audio_flags`]).
    pub flags: u16,
    /// Upper bound on one frame's decoded audio in bytes, as stored.
    pub max_size: u32,
}

impl AudioTrack {
    pub fn channels(&self) -> u16 {
        if self.flags & audio_flags::STEREO != 0 { 2 } else { 1 }
    }
}

#[derive(Clone, Debug)]
pub struct Header {
    /// Revision letter following `"BIK"` (`b'f'` ..= `b'i'`).
    pub revision: u8,
    pub frame_count: u32,
    pub largest_frame: u32,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    /// Video flag bits ([`flags`]).
    pub flags: u32,
    pub audio: Vec<AudioTrack>,
}

impl Header {
    pub fn has_alpha(&self) -> bool {
        self.flags & flags::ALPHA != 0
    }

    pub fn is_grayscale(&self) -> bool {
        self.flags & flags::GRAYSCALE != 0
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct IndexEntry {
    pub offset: u32,
    pub keyframe: bool,
}

pub(crate) fn parse(data: &[u8]) -> Result<(Header, Vec<IndexEntry>)> {
    if data.len() < FIXED_HEADER || &data[..3] != b"BIK" {
        return Err(Error::NotBink);
    }
    let revision = data[3];
    if !(b'f'..=b'i').contains(&revision) {
        return Err(Error::Revision(revision as char));
    }
    let word = |i: usize| u32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    let frame_count = word(2);
    let largest_frame = word(3);
    let index_len = word(4);
    let (width, height) = (word(5), word(6));
    let (fps_num, fps_den) = (word(7), word(8));
    let flags = word(9);
    let tracks = word(10) as usize;

    let bad = |msg: &str| Err(Error::Header(msg.into()));
    if frame_count == 0 {
        return bad("no frames");
    }
    if index_len < frame_count || index_len > 1_000_000 {
        return bad("frame index length out of range");
    }
    if width == 0 || height == 0 || width > 0x7FFF || height > 0x7FFF {
        return bad("frame size out of range");
    }
    if fps_num == 0 || fps_den == 0 {
        return bad("zero frame rate");
    }
    if tracks > 256 {
        return bad("too many audio tracks");
    }

    let tables = FIXED_HEADER + tracks * 12;
    let index_end = tables + (index_len as usize + 1) * 4;
    if data.len() < index_end {
        return Err(Error::Truncated);
    }
    let at = |off: usize| u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
    let audio = (0..tracks)
        .map(|t| {
            let rate_flags = at(FIXED_HEADER + tracks * 4 + t * 4);
            AudioTrack {
                id: at(FIXED_HEADER + tracks * 8 + t * 4),
                sample_rate: rate_flags & 0xFFFF,
                flags: (rate_flags >> 16) as u16,
                max_size: at(FIXED_HEADER + t * 4),
            }
        })
        .collect();

    let mut index = Vec::with_capacity(frame_count as usize + 1);
    for i in 0..=frame_count as usize {
        let raw = at(tables + i * 4);
        index.push(IndexEntry { offset: raw & !1, keyframe: raw & 1 != 0 });
    }
    for pair in index.windows(2) {
        if pair[1].offset < pair[0].offset || pair[1].offset as usize > data.len() {
            return bad("frame offsets out of order or past the end of the file");
        }
    }
    if (index[0].offset as usize) < index_end {
        return bad("first frame overlaps the header");
    }

    let header = Header { revision, frame_count, largest_frame, width, height, fps_num, fps_den, flags, audio };
    Ok((header, index))
}

/// Splits one frame's bytes into its audio packets (one per track, possibly
/// empty) and the video packet.
pub(crate) fn split_frame(frame: &[u8], tracks: usize) -> Result<(Vec<&[u8]>, &[u8])> {
    let mut pos = 0;
    let mut audio = Vec::with_capacity(tracks);
    for _ in 0..tracks {
        let size = frame.get(pos..pos + 4).ok_or(Error::Truncated)?;
        let size = u32::from_le_bytes(size.try_into().unwrap()) as usize;
        pos += 4;
        let packet = frame.get(pos..pos + size).ok_or(Error::Truncated)?;
        audio.push(packet);
        pos += size;
    }
    Ok((audio, &frame[pos..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic(tracks: &[(u32, u16)], frames: &[(&[u8], bool)]) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(b"BIKi");
        let words = [0, frames.len() as u32, 0, frames.len() as u32, 64, 32, 30, 1, 0, tracks.len() as u32];
        for w in words {
            d.extend_from_slice(&w.to_le_bytes());
        }
        for _ in tracks {
            d.extend_from_slice(&0x1000u32.to_le_bytes());
        }
        for &(rate, fl) in tracks {
            d.extend_from_slice(&(rate | (fl as u32) << 16).to_le_bytes());
        }
        for (i, _) in tracks.iter().enumerate() {
            d.extend_from_slice(&(i as u32).to_le_bytes());
        }
        let mut off = d.len() + (frames.len() + 1) * 4;
        for (body, key) in frames {
            d.extend_from_slice(&(off as u32 | *key as u32).to_le_bytes());
            off += body.len();
        }
        d.extend_from_slice(&(off as u32).to_le_bytes());
        for (body, _) in frames {
            d.extend_from_slice(body);
        }
        let size = d.len() as u32 - 8;
        d[4..8].copy_from_slice(&size.to_le_bytes());
        d
    }

    #[test]
    fn header_and_index() {
        let f0 = [0u8, 0, 0, 0, 0xAA, 0xBB];
        // Frame offsets are even (bit 0 is the keyframe flag).
        let f1 = [2u8, 0, 0, 0, 1, 2, 0xCC, 0xDD];
        let data = synthetic(&[(44100, audio_flags::DCT | audio_flags::STEREO)], &[(&f0, true), (&f1, false)]);
        let (h, index) = parse(&data).unwrap();
        assert_eq!((h.width, h.height, h.frame_count), (64, 32, 2));
        assert_eq!(h.audio.len(), 1);
        assert_eq!(h.audio[0].sample_rate, 44100);
        assert_eq!(h.audio[0].channels(), 2);
        assert!(index[0].keyframe && !index[1].keyframe);
        let frame1 = &data[index[1].offset as usize..index[2].offset as usize];
        let (audio, video) = split_frame(frame1, 1).unwrap();
        assert_eq!(audio[0], &[1, 2]);
        assert_eq!(video, &[0xCC, 0xDD]);
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(parse(b"RIFF0000"), Err(Error::NotBink)));
        let mut data = synthetic(&[], &[(&[0], true)]);
        data[3] = b'b';
        assert!(matches!(parse(&data), Err(Error::Revision('b'))));
        let data = synthetic(&[], &[(&[0], true)]);
        assert!(matches!(parse(&data[..50]), Err(Error::Truncated)));
    }
}
