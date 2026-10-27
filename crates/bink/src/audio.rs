//! Bink audio (the DCT based variant).
//!
//! An audio packet starts with the number of bytes it decodes to (16-bit
//! samples, all channels) followed by transform blocks, each aligned to 32
//! bits. A block codes `N` frequency coefficients per channel (`N` = 512,
//! 1024 or 2048 by sample rate): two floats, one quantiser per critical band
//! and run-length grouped integers. An inverse DCT turns them into `N`
//! samples; the last `N / 16` overlap the start of the next block and are
//! cross-faded into it, so every block yields `N - N / 16` samples per
//! channel.
//!
//! The FFT based variant of the codec is not implemented because no Portal 2
//! file uses it (see `docs/re/bink.md`).

use crate::bits::Bits;
use crate::container::audio_flags;
use crate::tables::{AUDIO_BAND_EDGES, AUDIO_QUANT, AUDIO_RUNS};
use crate::{AudioTrack, Error, Result};

/// Streaming decoder for one audio track. Feed it the track's packets in
/// frame order; call [`AudioDecoder::reset`] after seeking.
pub struct AudioDecoder {
    sample_rate: u32,
    channels: usize,
    /// Transform length per channel.
    len: usize,
    /// Start of every quantiser band in coefficients, then `len`.
    bands: Vec<usize>,
    /// Output gain applied after the transform.
    gain: f32,
    dct: Dct,
    coefs: Vec<f32>,
    /// One decoded block, interleaved.
    block: Vec<i16>,
    /// The overlapping end of the previous block, interleaved.
    tail: Vec<i16>,
    first: bool,
}

impl AudioDecoder {
    pub fn new(track: &AudioTrack) -> Result<Self> {
        if track.flags & audio_flags::FFT != 0 {
            // TODO: implement the FFT variant if a file ever needs it.
            return Err(Error::Audio("FFT based Bink audio is not supported"));
        }
        if track.flags & audio_flags::DCT == 0 {
            return Err(Error::Audio("track does not use a Bink audio codec"));
        }
        if track.sample_rate == 0 {
            return Err(Error::Audio("zero sample rate"));
        }
        let channels = track.channels() as usize;
        let len = match track.sample_rate {
            ..22050 => 512,
            22050..44100 => 1024,
            _ => 2048,
        };
        // Bands follow the critical band edges up to half the sample rate.
        let nyquist = track.sample_rate.div_ceil(2) as usize;
        let count = AUDIO_BAND_EDGES.iter().position(|&e| e as usize >= nyquist).map_or(25, |i| i + 1);
        let mut bands: Vec<usize> = (0..count)
            .map(|i| {
                let edge = if i == 0 { 0 } else { AUDIO_BAND_EDGES[i - 1] as usize };
                2 * (len / 2 * edge / nyquist).max(1)
            })
            .collect();
        bands.push(len);
        Ok(Self {
            sample_rate: track.sample_rate,
            channels,
            len,
            bands,
            gain: (2.0 / (len as f64).sqrt()) as f32,
            dct: Dct::new(len),
            coefs: vec![0.0; len],
            block: vec![0; len * channels],
            tail: vec![0; len / 16 * channels],
            first: true,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Forgets the overlap from earlier packets (after a seek).
    pub fn reset(&mut self) {
        self.first = true;
    }

    /// Decodes one audio packet, appending interleaved samples to `out`.
    /// Returns the number of samples appended.
    pub fn decode(&mut self, packet: &[u8], out: &mut Vec<i16>) -> Result<usize> {
        let Some((size, data)) = packet.split_first_chunk::<4>() else {
            return Err(Error::Audio("packet too short"));
        };
        let mut wanted = u32::from_le_bytes(*size) as usize / 2;
        let start = out.len();
        let mut bits = Bits::new(data);
        while wanted > 0 {
            if bits.position() >= data.len() * 8 {
                return Err(Error::Audio("packet ends early"));
            }
            self.decode_block(&mut bits);
            bits.align32();
            let produced = &self.block[..(self.len - self.len / 16) * self.channels];
            let take = produced.len().min(wanted);
            out.extend_from_slice(&produced[..take]);
            wanted -= take;
        }
        Ok(out.len() - start)
    }

    fn decode_block(&mut self, bits: &mut Bits) {
        bits.skip(2);
        for ch in 0..self.channels {
            self.read_coefficients(bits);
            self.dct.run(&self.coefs);
            for (k, &v) in self.dct.output().iter().enumerate() {
                let s = (v as f32 * self.gain).round_ties_even();
                self.block[k * self.channels + ch] = s.clamp(-32768.0, 32767.0) as i16;
            }
        }
        // Cross-fade the start of this block from the end of the previous
        // one in 16-bit integer steps, then keep this block's end.
        let overlap = self.tail.len();
        if !self.first {
            let shift = overlap.trailing_zeros();
            for (i, (cur, &prev)) in self.block[..overlap].iter_mut().zip(&self.tail).enumerate() {
                let diff = cur.wrapping_sub(prev) as i32;
                let v = prev as i32 + ((i as i32 * diff) >> shift);
                *cur = v.clamp(-32768, 32767) as i16;
            }
        }
        self.first = false;
        let end = self.block.len();
        self.tail.copy_from_slice(&self.block[end - overlap..]);
    }

    fn read_coefficients(&mut self, bits: &mut Bits) {
        let c = &mut self.coefs;
        c[0] = read_float(bits);
        c[1] = read_float(bits);
        let mut quant = [0f32; 25];
        for q in &mut quant[..self.bands.len() - 1] {
            *q = AUDIO_QUANT[(bits.read(8) as usize).min(95)];
        }
        let (mut band, mut q) = (0, 0f32);
        let mut i = 2;
        while i < self.len {
            let run = if bits.bit() { AUDIO_RUNS[bits.read(4) as usize] } else { 1 };
            let end = (i + run * 8).min(self.len);
            let width = bits.read(4);
            if width == 0 {
                c[i..end].fill(0.0);
                i = end;
                while self.bands[band] < end {
                    q = quant[band];
                    band += 1;
                }
                continue;
            }
            while i < end {
                if self.bands[band] == i {
                    q = quant[band];
                    band += 1;
                }
                let v = bits.read(width);
                c[i] = if v == 0 {
                    0.0
                } else if bits.bit() {
                    -q * v as f32
                } else {
                    q * v as f32
                };
                i += 1;
            }
        }
    }
}

/// A float stored as 5-bit exponent, 23-bit mantissa and sign.
fn read_float(bits: &mut Bits) -> f32 {
    let exp = bits.read(5) as i32;
    let mantissa = bits.read(23) as f32;
    let v = mantissa * 2f32.powi(exp - 23);
    if bits.bit() { -v } else { v }
}

/// `out[k] = sum(c[j] * cos(pi * j * (k + 1/2) / n))` for `k < n`, computed
/// as the real part of a zero-padded complex FFT of length `2n`.
struct Dct {
    n: usize,
    /// `exp(i * pi * j / 2n)` for `j < n`.
    pre: Vec<(f64, f64)>,
    /// `exp(i * 2 * pi * t / 2n)` for `t < n`.
    roots: Vec<(f64, f64)>,
    work: Vec<(f64, f64)>,
    out: Vec<f64>,
}

impl Dct {
    fn new(n: usize) -> Self {
        let unit = |angle: f64| (angle.cos(), angle.sin());
        let step = std::f64::consts::PI / n as f64;
        Self {
            n,
            pre: (0..n).map(|j| unit(step * j as f64 / 2.0)).collect(),
            roots: (0..n).map(|t| unit(step * t as f64)).collect(),
            work: vec![(0.0, 0.0); 2 * n],
            out: vec![0.0; n],
        }
    }

    fn output(&self) -> &[f64] {
        &self.out
    }

    fn run(&mut self, c: &[f32]) {
        let size = 2 * self.n;
        let bits = size.trailing_zeros();
        self.work.fill((0.0, 0.0));
        for (j, (&v, &(re, im))) in c.iter().zip(&self.pre).enumerate() {
            // Place in bit-reversed order for the in-place FFT below.
            let r = j.reverse_bits() >> (usize::BITS - bits);
            self.work[r] = (v as f64 * re, v as f64 * im);
        }
        let mut len = 2;
        while len <= size {
            let half = len / 2;
            let stride = size / len;
            for start in (0..size).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = self.roots[k * stride];
                    let (ar, ai) = self.work[start + k];
                    let (br, bi) = self.work[start + k + half];
                    let (tr, ti) = (br * wr - bi * wi, br * wi + bi * wr);
                    self.work[start + k] = (ar + tr, ai + ti);
                    self.work[start + k + half] = (ar - tr, ai - ti);
                }
            }
            len *= 2;
        }
        for (o, w) in self.out.iter_mut().zip(&self.work) {
            *o = w.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dct_matches_definition() {
        let n = 16;
        let c: Vec<f32> = (0..n).map(|j| ((j * 7 + 3) % 11) as f32 - 5.0).collect();
        let mut dct = Dct::new(n);
        dct.run(&c);
        for k in 0..n {
            let direct: f64 = (0..n)
                .map(|j| c[j] as f64 * (std::f64::consts::PI * j as f64 * (k as f64 + 0.5) / n as f64).cos())
                .sum();
            assert!((dct.output()[k] - direct).abs() < 1e-9, "k={k}");
        }
    }

    #[test]
    fn bands_for_common_rates() {
        let track = |rate| AudioTrack { id: 0, sample_rate: rate, flags: audio_flags::DCT, max_size: 0 };
        let d = AudioDecoder::new(&track(44100)).unwrap();
        assert_eq!(d.len, 2048);
        assert_eq!(d.bands.len(), 26);
        assert_eq!(&d.bands[..3], &[2, 8, 18]);
        assert_eq!(*d.bands.last().unwrap(), 2048);
        // 11025 Hz Nyquist: the last band ends at the 12000 Hz edge.
        let d = AudioDecoder::new(&track(22050)).unwrap();
        assert_eq!((d.len, d.bands.len()), (1024, 24));
        let d = AudioDecoder::new(&track(11025)).unwrap();
        // 5513 Hz Nyquist: the band ending at 5300 Hz is the last full one.
        assert_eq!((d.len, d.bands.len()), (512, 21));
    }

    #[test]
    fn rejects_other_codecs() {
        let track = |flags| AudioTrack { id: 0, sample_rate: 44100, flags, max_size: 0 };
        assert!(AudioDecoder::new(&track(audio_flags::FFT | audio_flags::STEREO)).is_err());
        assert!(AudioDecoder::new(&track(audio_flags::STEREO)).is_err());
    }
}
