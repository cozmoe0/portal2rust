//! Bink 1 video decoding.
//!
//! Each plane of a frame (alpha, luma, then the two chroma planes) is coded on
//! its own as a grid of 8x8 blocks. Per-block values are not interleaved with
//! the blocks: they are grouped into nine value streams ("bundles": block
//! types, colours, motion vectors, DC values, ...) whose next chunk is read at
//! the start of a block row once the previous chunk is used up. Bits that
//! belong to a single block (coefficients, run flags) follow in the main
//! stream. Pixel arithmetic wraps modulo 256 like the reference decoder.

use std::num::Wrapping;

use crate::bits::Bits;
use crate::frame::{Frame, Plane};
use crate::tables::{BLOCK_RUNS, INTER_QUANT, INTRA_QUANT, RUN_PATTERNS, SCAN, TREE_CODES, TREE_LENS};

type DecodeResult<T> = std::result::Result<T, &'static str>;

/// Block kinds, as coded in the block-type bundle.
mod block {
    /// Copy of the co-located block of the previous frame.
    pub const SKIP: u8 = 0;
    /// A 16x16 block coded at half resolution (kind from the sub-type bundle).
    pub const SCALED: u8 = 1;
    /// Motion-compensated copy of the previous frame.
    pub const MOTION: u8 = 2;
    /// Runs of colours along one of 16 pixel orders.
    pub const RUN: u8 = 3;
    /// Motion-compensated copy plus a sparse pixel residue.
    pub const RESIDUE: u8 = 4;
    /// DCT coded.
    pub const INTRA: u8 = 5;
    /// A single colour.
    pub const FILL: u8 = 6;
    /// Motion-compensated copy plus a DCT coded difference.
    pub const INTER: u8 = 7;
    /// Two colours chosen per pixel by a bit mask.
    pub const PATTERN: u8 = 8;
    /// 64 colours.
    pub const RAW: u8 = 9;
}

/// Bundle slots, in the order their headers and chunks appear.
const TYPES: usize = 0;
const SUB_TYPES: usize = 1;
const COLORS: usize = 2;
const PATTERNS: usize = 3;
const MOTION_X: usize = 4;
const MOTION_Y: usize = 5;
const INTRA_DC: usize = 6;
const INTER_DC: usize = 7;
const RUNS: usize = 8;

/// Lookup tables for the 16 code books, indexed by the next 7 stream bits.
/// Each entry is `code length << 4 | symbol index`.
static BOOK_LOOKUP: [[u8; 128]; 16] = build_lookup();

const fn build_lookup() -> [[u8; 128]; 16] {
    let mut out = [[0u8; 128]; 16];
    let mut book = 0;
    while book < 16 {
        let mut sym = 0;
        while sym < 16 {
            let len = TREE_LENS[book][sym] as usize;
            let code = TREE_CODES[book][sym] as usize;
            let mut high = 0;
            while high < 128 >> len {
                out[book][code | high << len] = (len as u8) << 4 | sym as u8;
                high += 1;
            }
            sym += 1;
        }
        book += 1;
    }
    out
}

/// One of the static code books plus the order in which its 16 code words
/// map to values, as declared at the start of every plane.
#[derive(Clone, Copy)]
struct Codebook {
    book: usize,
    values: [u8; 16],
}

impl Default for Codebook {
    fn default() -> Self {
        Self { book: 0, values: std::array::from_fn(|i| i as u8) }
    }
}

impl Codebook {
    fn read(bits: &mut Bits) -> Self {
        let mut cb = Self { book: bits.read(4) as usize, ..Self::default() };
        if cb.book == 0 {
            return cb;
        }
        if bits.bit() {
            // Explicit leading values; the unnamed ones follow in ascending order.
            let named = bits.read(3) as usize + 1;
            let mut seen = [false; 16];
            for slot in 0..named {
                let v = bits.read(4) as u8;
                cb.values[slot] = v;
                seen[v as usize] = true;
            }
            let rest = (0..16u8).filter(|&v| !seen[v as usize]);
            for (slot, v) in (named..16).zip(rest) {
                cb.values[slot] = v;
            }
        } else {
            // Shuffle the identity order: merge neighbouring runs of 1, 2, 4
            // and 8 values, each stream bit picking the left (0) or right (1)
            // run's next value.
            let passes = bits.read(2) + 1;
            for pass in 0..passes {
                let run = 1usize << pass;
                let src = cb.values;
                for start in (0..16).step_by(2 * run) {
                    let (mut l, mut r) = (start, start + run);
                    for slot in start..start + 2 * run {
                        let take_right = if l == start + run {
                            true
                        } else if r == start + 2 * run {
                            false
                        } else {
                            bits.bit()
                        };
                        if take_right {
                            cb.values[slot] = src[r];
                            r += 1;
                        } else {
                            cb.values[slot] = src[l];
                            l += 1;
                        }
                    }
                }
            }
        }
        cb
    }

    #[inline]
    fn decode(&self, bits: &mut Bits) -> u8 {
        let entry = BOOK_LOOKUP[self.book][bits.peek(7) as usize];
        bits.skip((entry >> 4) as u32);
        self.values[(entry & 15) as usize]
    }
}

/// A value stream, refilled chunk by chunk.
#[derive(Default)]
struct Bundle {
    values: Vec<i16>,
    pos: usize,
    count_bits: u32,
    book: Codebook,
    /// A zero-length chunk ends the stream for the rest of the plane.
    finished: bool,
}

impl Bundle {
    fn start_plane(&mut self, max_chunk: usize) {
        self.values.clear();
        self.pos = 0;
        self.finished = false;
        self.count_bits = usize::BITS - max_chunk.leading_zeros();
    }

    /// Reads the next chunk length if the current chunk is used up, else 0.
    fn begin_chunk(&mut self, bits: &mut Bits) -> usize {
        if self.finished || self.pos < self.values.len() {
            return 0;
        }
        self.values.clear();
        self.pos = 0;
        let n = bits.read(self.count_bits) as usize;
        self.finished = n == 0;
        n
    }

    #[inline]
    fn next(&mut self) -> i16 {
        let v = self.values.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        v
    }
}

/// Entries of the work list used by the progressive coefficient coders, each
/// naming the scan index of its first coefficient.
#[derive(Clone, Copy, PartialEq)]
enum Group {
    Empty,
    /// Four coefficients, followed by a `Sixteen` group right after them.
    Twenty(u8),
    /// Splits into four `Four` groups.
    Sixteen(u8),
    /// Four coefficients, each coded now or deferred as a `Single`.
    Four(u8),
    Single(u8),
}

/// Work list with room to grow in both directions: deferred single
/// coefficients are put in front, split groups are appended.
struct WorkList {
    items: [Group; 128],
    head: usize,
    tail: usize,
}

impl WorkList {
    fn new(initial: &[Group]) -> Self {
        let mut list = Self { items: [Group::Empty; 128], head: 64, tail: 64 };
        for &g in initial {
            list.push_back(g);
        }
        list
    }

    fn push_back(&mut self, g: Group) {
        self.items[self.tail] = g;
        self.tail += 1;
    }

    fn push_front(&mut self, g: Group) {
        self.head -= 1;
        self.items[self.head] = g;
    }
}

/// What a coefficient coder does once a coefficient becomes significant.
trait CoefSink {
    /// Called for a newly significant coefficient at scan index `idx`;
    /// returns false to stop decoding.
    fn set(&mut self, bits: &mut Bits, idx: usize) -> bool;
}

/// One significance pass, shared by DCT and residue coding: every pending
/// list entry reads a bit, and significant groups are split or coded.
/// Entries put in front are only visited by the next pass, appended ones by
/// this one. Returns false once the sink stops decoding.
fn coefficient_pass<S: CoefSink>(bits: &mut Bits, list: &mut WorkList, sink: &mut S) -> bool {
    let mut i = list.head;
    while i < list.tail {
        let g = list.items[i];
        if g == Group::Empty || !bits.bit() {
            i += 1;
            continue;
        }
        let first = match g {
            Group::Twenty(c) => {
                // Code its first four coefficients now; the rest stays as a
                // sixteen-coefficient group in the same slot.
                list.items[i] = Group::Sixteen(c + 4);
                c
            }
            Group::Sixteen(c) => {
                list.items[i] = Group::Four(c);
                for k in 1..4 {
                    list.push_back(Group::Four(c + 4 * k));
                }
                continue;
            }
            Group::Four(c) => {
                list.items[i] = Group::Empty;
                i += 1;
                c
            }
            Group::Single(c) => {
                list.items[i] = Group::Empty;
                i += 1;
                if !sink.set(bits, c as usize) {
                    return false;
                }
                continue;
            }
            Group::Empty => unreachable!(),
        };
        for c in first..first + 4 {
            if bits.bit() {
                list.push_front(Group::Single(c));
            } else if !sink.set(bits, c as usize) {
                return false;
            }
        }
    }
    true
}

/// DCT coefficients: magnitudes start at the current bit plane.
struct DctSink<'a> {
    coefs: &'a mut [i16; 64],
    plane: u32,
}

impl CoefSink for DctSink<'_> {
    fn set(&mut self, bits: &mut Bits, idx: usize) -> bool {
        let magnitude = (1 << self.plane | bits.read(self.plane)) as i16;
        self.coefs[SCAN[idx] as usize] = if bits.bit() { -magnitude } else { magnitude };
        true
    }
}

/// Reads the AC coefficients of a DCT block (raster order, DC already set)
/// and returns the quantiser index that follows them.
fn read_dct(bits: &mut Bits, coefs: &mut [i16; 64]) -> usize {
    let mut list = WorkList::new(&[
        Group::Twenty(4),
        Group::Twenty(24),
        Group::Twenty(44),
        Group::Single(1),
        Group::Single(2),
        Group::Single(3),
    ]);
    let planes = bits.read(4);
    for plane in (0..planes).rev() {
        coefficient_pass(bits, &mut list, &mut DctSink { coefs, plane });
    }
    bits.read(4) as usize
}

/// Residue values: signed bytes in scan order. Earlier coefficients are
/// refined at every lower mask level. Arithmetic wraps like the reference.
struct ResidueSink<'a> {
    values: &'a mut [i8; 64],
    nonzero: [u8; 64],
    count: usize,
    mask: i8,
    budget: i32,
}

impl ResidueSink<'_> {
    fn spend(&mut self) -> bool {
        self.budget -= 1;
        self.budget >= 0
    }
}

impl CoefSink for ResidueSink<'_> {
    fn set(&mut self, bits: &mut Bits, idx: usize) -> bool {
        self.nonzero[self.count] = idx as u8;
        self.count += 1;
        self.values[idx] = if bits.bit() { self.mask.wrapping_neg() } else { self.mask };
        self.spend()
    }
}

/// Reads a residue block; `budget + 1` is the number of coefficient updates.
fn read_residue(bits: &mut Bits, values: &mut [i8; 64], budget: i32) {
    let mut list = WorkList::new(&[Group::Twenty(4), Group::Twenty(24), Group::Twenty(44), Group::Four(0)]);
    let top = bits.read(3);
    let mut sink = ResidueSink { values, nonzero: [0; 64], count: 0, mask: 0, budget };
    for level in (0..=top).rev() {
        sink.mask = (1u8 << level) as i8;
        for k in 0..sink.count {
            if !bits.bit() {
                continue;
            }
            let idx = sink.nonzero[k] as usize;
            let v = sink.values[idx];
            sink.values[idx] = if v < 0 { v.wrapping_sub(sink.mask) } else { v.wrapping_add(sink.mask) };
            if !sink.spend() {
                return;
            }
        }
        if !coefficient_pass(bits, &mut list, &mut sink) {
            return;
        }
    }
}

type W = Wrapping<i32>;

/// One-dimensional pass of Bink's integer inverse DCT.
#[inline(always)]
fn idct8(s: [W; 8]) -> [W; 8] {
    let scale = |k: i32, v: W| Wrapping((Wrapping(k) * v).0 >> 11);
    let e0 = s[0] + s[4];
    let e1 = s[0] - s[4];
    let e2 = s[2] + s[6];
    let e3 = scale(2896, s[2] - s[6]) - e2;
    let o0 = s[5] + s[3];
    let o1 = s[5] - s[3];
    let o2 = s[1] + s[7];
    let o3 = s[1] - s[7];
    let b0 = o2 + o0;
    let b1 = scale(3784, o3 + o1);
    let b2 = scale(-5352, o1) + b1 - b0;
    let b3 = scale(2896, o2 - o0) - b2;
    let b4 = scale(2217, o3) + b3 - b1;
    [
        e0 + e2 + b0,
        e1 + e3 + b2,
        e1 - e3 + b3,
        e0 - e2 - b4,
        e0 - e2 + b4,
        e1 - e3 - b3,
        e1 + e3 - b2,
        e0 + e2 - b0,
    ]
}

/// Dequantises (raster order) and inverse transforms a block. The results
/// are pixel offsets; only their low eight bits matter.
fn idct(coefs: &[i16; 64], quant: &[u32; 64]) -> [u8; 64] {
    let mut tmp = [Wrapping(0i32); 64];
    for col in 0..8 {
        let dq = |row: usize| {
            let i = row * 8 + col;
            Wrapping((Wrapping(coefs[i] as i32) * Wrapping(quant[i] as i32)).0 >> 11)
        };
        let s: [W; 8] = std::array::from_fn(dq);
        let out = if (1..8).all(|row| coefs[row * 8 + col] == 0) { [s[0]; 8] } else { idct8(s) };
        for (row, v) in out.into_iter().enumerate() {
            tmp[row * 8 + col] = v;
        }
    }
    let mut px = [0u8; 64];
    for row in 0..8 {
        let out = idct8(tmp[row * 8..row * 8 + 8].try_into().unwrap());
        for (col, v) in out.into_iter().enumerate() {
            px[row * 8 + col] = ((v + Wrapping(127)).0 >> 8) as u8;
        }
    }
    px
}

/// Writes an 8x8 block.
fn put8(p: &mut Plane, x: usize, y: usize, px: &[u8; 64]) {
    for (r, src) in px.as_chunks::<8>().0.iter().enumerate() {
        let o = (y + r) * p.width + x;
        p.data[o..o + 8].copy_from_slice(src);
    }
}

/// Writes an 8x8 block at twice its size, clipped to the plane.
fn put16(p: &mut Plane, x: usize, y: usize, px: &[u8; 64]) {
    let w = (p.width - x).min(16);
    for r in 0..(p.height - y).min(16) {
        let o = (y + r) * p.width + x;
        for (c, d) in p.data[o..o + w].iter_mut().enumerate() {
            *d = px[(r / 2) * 8 + c / 2];
        }
    }
}

fn fill(p: &mut Plane, x: usize, y: usize, size: usize, v: u8) {
    let w = (p.width - x).min(size);
    for r in 0..(p.height - y).min(size) {
        let o = (y + r) * p.width + x;
        p.data[o..o + w].fill(v);
    }
}

/// Reads an 8x8 block of the previous frame starting at linear offset `src`.
fn fetch8(prev: &Plane, src: usize) -> [u8; 64] {
    let mut px = [0u8; 64];
    for (r, dst) in px.as_chunks_mut::<8>().0.iter_mut().enumerate() {
        let o = src + r * prev.width;
        dst.copy_from_slice(&prev.data[o..o + 8]);
    }
    px
}

/// Decodes single planes; reused for all planes and frames.
#[derive(Default)]
pub(crate) struct PlaneDecoder {
    bundles: [Bundle; 9],
    /// Code books for a colour's high nibble, selected by the previous one.
    color_books: [Codebook; 16],
    color_high: u8,
    /// Revisions before `i` store colours as sign and magnitude.
    signed_colors: bool,
}

impl PlaneDecoder {
    fn next(&mut self, bundle: usize) -> i16 {
        self.bundles[bundle].next()
    }

    fn color(&mut self) -> u8 {
        self.bundles[COLORS].next() as u8
    }

    fn start_plane(&mut self, bits: &mut Bits, width: usize) {
        // Chunk lengths are stored with just enough bits for these maxima.
        let bw = width / 8;
        let row = bw + 511;
        let limits = [row, width / 16 + 511, bw * 64 + 511, bw * 8 + 511, row, row, row, row, bw * 48 + 511];
        for (b, limit) in self.bundles.iter_mut().zip(limits) {
            b.start_plane(limit);
        }
        // Code book headers in bundle order; the DC bundles have none and the
        // colour bundle's own book comes after the 16 high-nibble books.
        self.bundles[TYPES].book = Codebook::read(bits);
        self.bundles[SUB_TYPES].book = Codebook::read(bits);
        for b in &mut self.color_books {
            *b = Codebook::read(bits);
        }
        self.color_high = 0;
        for slot in [COLORS, PATTERNS, MOTION_X, MOTION_Y, RUNS] {
            self.bundles[slot].book = Codebook::read(bits);
        }
    }

    /// Reads the next chunk of every bundle that ran dry.
    fn refill(&mut self, bits: &mut Bits) -> DecodeResult<()> {
        self.read_block_types(bits, TYPES)?;
        self.read_block_types(bits, SUB_TYPES)?;
        self.read_colors(bits);
        self.read_patterns(bits);
        self.read_motion(bits, MOTION_X);
        self.read_motion(bits, MOTION_Y);
        self.read_dc(bits, INTRA_DC, false)?;
        self.read_dc(bits, INTER_DC, true)?;
        self.read_runs(bits);
        Ok(())
    }

    fn read_block_types(&mut self, bits: &mut Bits, slot: usize) -> DecodeResult<()> {
        let b = &mut self.bundles[slot];
        let n = b.begin_chunk(bits);
        if n == 0 {
            return Ok(());
        }
        if bits.bit() {
            let v = bits.read(4) as i16;
            b.values.resize(n, v);
            return Ok(());
        }
        let mut last = 0;
        while b.values.len() < n {
            let sym = b.book.decode(bits);
            if sym < 12 {
                last = sym as i16;
                b.values.push(last);
            } else {
                let run = BLOCK_RUNS[sym as usize - 12];
                if b.values.len() + run > n {
                    return Err("block type run overflows its chunk");
                }
                b.values.resize(b.values.len() + run, last);
            }
        }
        Ok(())
    }

    fn read_colors(&mut self, bits: &mut Bits) {
        let n = self.bundles[COLORS].begin_chunk(bits);
        if n == 0 {
            return;
        }
        let repeat = bits.bit();
        for _ in 0..if repeat { 1 } else { n } {
            self.color_high = self.color_books[self.color_high as usize].decode(bits);
            let b = &mut self.bundles[COLORS];
            let mut v = self.color_high << 4 | b.book.decode(bits);
            if self.signed_colors {
                let magnitude = v & 0x7F;
                v = if v & 0x80 == 0 { 0x80 + magnitude } else { 0x80 - magnitude };
            }
            b.values.push(v as i16);
        }
        let b = &mut self.bundles[COLORS];
        let v = b.values[0];
        b.values.resize(n, v);
    }

    fn read_patterns(&mut self, bits: &mut Bits) {
        let b = &mut self.bundles[PATTERNS];
        let n = b.begin_chunk(bits);
        for _ in 0..n {
            let low = b.book.decode(bits);
            let high = b.book.decode(bits);
            b.values.push((low | high << 4) as i16);
        }
    }

    fn read_motion(&mut self, bits: &mut Bits, slot: usize) {
        let b = &mut self.bundles[slot];
        let n = b.begin_chunk(bits);
        if n == 0 {
            return;
        }
        let repeat = bits.bit();
        for _ in 0..if repeat { 1 } else { n } {
            let mut v = if repeat { bits.read(4) } else { b.book.decode(bits) as u32 } as i16;
            if v != 0 && bits.bit() {
                v = -v;
            }
            b.values.push(v);
        }
        let v = b.values[0];
        b.values.resize(n, v);
    }

    /// DC values: a start value, then groups of up to eight deltas sharing a
    /// bit width.
    fn read_dc(&mut self, bits: &mut Bits, slot: usize, signed: bool) -> DecodeResult<()> {
        let b = &mut self.bundles[slot];
        let n = b.begin_chunk(bits);
        if n == 0 {
            return Ok(());
        }
        let mut v = bits.read(if signed { 10 } else { 11 }) as i32;
        if signed && v != 0 && bits.bit() {
            v = -v;
        }
        b.values.push(v as i16);
        let mut left = n - 1;
        while left > 0 {
            let group = left.min(8);
            let width = bits.read(4);
            for _ in 0..group {
                if width != 0 {
                    let mut d = bits.read(width) as i32;
                    if d != 0 && bits.bit() {
                        d = -d;
                    }
                    v += d;
                    if !(-32768..=32767).contains(&v) {
                        return Err("DC value out of range");
                    }
                }
                b.values.push(v as i16);
            }
            left -= group;
        }
        Ok(())
    }

    fn read_runs(&mut self, bits: &mut Bits) {
        let b = &mut self.bundles[RUNS];
        let n = b.begin_chunk(bits);
        if n == 0 {
            return;
        }
        if bits.bit() {
            let v = bits.read(4) as i16;
            b.values.resize(n, v);
        } else {
            for _ in 0..n {
                let v = b.book.decode(bits) as i16;
                b.values.push(v);
            }
        }
    }

    /// Linear offset of the block a motion vector points at.
    fn motion_source(&mut self, prev: &Plane, x: usize, y: usize) -> DecodeResult<usize> {
        let dx = self.next(MOTION_X) as isize;
        let dy = self.next(MOTION_Y) as isize;
        let stride = prev.width as isize;
        // Like the reference decoder this addresses the plane linearly, so a
        // vector may reach past a side edge into the neighbouring row.
        let src = (y as isize + dy) * stride + x as isize + dx;
        if src < 0 || src + 7 * stride + 8 > prev.data.len() as isize {
            return Err("motion vector outside the reference plane");
        }
        Ok(src as usize)
    }

    fn run_block(&mut self, bits: &mut Bits, px: &mut [u8; 64]) -> DecodeResult<()> {
        let order = &RUN_PATTERNS[bits.read(4) as usize];
        let mut i = 0;
        while i < 63 {
            let run = self.next(RUNS) as usize + 1;
            if i + run > 64 {
                return Err("run block overflows");
            }
            if bits.bit() {
                let c = self.color();
                for &p in &order[i..i + run] {
                    px[p as usize] = c;
                }
            } else {
                for &p in &order[i..i + run] {
                    px[p as usize] = self.color();
                }
            }
            i += run;
        }
        if i == 63 {
            px[order[63] as usize] = self.color();
        }
        Ok(())
    }

    fn pattern_block(&mut self, px: &mut [u8; 64]) {
        let colors = [self.color(), self.color()];
        for row in px.as_chunks_mut::<8>().0 {
            let mask = self.next(PATTERNS);
            for (c, p) in row.iter_mut().enumerate() {
                *p = colors[(mask >> c) as usize & 1];
            }
        }
    }

    fn raw_block(&mut self, px: &mut [u8; 64]) {
        for p in px.iter_mut() {
            *p = self.color();
        }
    }

    fn intra_block(&mut self, bits: &mut Bits) -> [u8; 64] {
        let mut coefs = [0i16; 64];
        coefs[0] = self.next(INTRA_DC);
        let q = read_dct(bits, &mut coefs);
        idct(&coefs, &INTRA_QUANT[q])
    }

    /// A 16x16 block at an even block row.
    fn scaled_block(&mut self, bits: &mut Bits, cur: &mut Plane, x: usize, y: usize) -> DecodeResult<()> {
        let mut px = [0u8; 64];
        match self.next(SUB_TYPES) as u8 {
            block::RUN => self.run_block(bits, &mut px)?,
            block::INTRA => px = self.intra_block(bits),
            block::FILL => {
                let c = self.color();
                fill(cur, x, y, 16, c);
                return Ok(());
            }
            block::PATTERN => self.pattern_block(&mut px),
            block::RAW => self.raw_block(&mut px),
            _ => return Err("invalid scaled block type"),
        }
        put16(cur, x, y, &px);
        Ok(())
    }

    /// Decodes one plane from `data` into `cur`, predicting from `prev`.
    /// Returns the number of bytes used (planes end on 32-bit boundaries).
    pub fn decode(&mut self, data: &[u8], cur: &mut Plane, prev: &Plane, signed_colors: bool)
    -> DecodeResult<usize> {
        let mut bits = Bits::new(data);
        self.signed_colors = signed_colors;
        self.start_plane(&mut bits, cur.width);
        let (bw, bh) = (cur.width / 8, cur.height / 8);
        for by in 0..bh {
            self.refill(&mut bits)?;
            let y = by * 8;
            let mut bx = 0;
            while bx < bw {
                let x = bx * 8;
                let kind = self.next(TYPES) as u8;
                if kind == block::SCALED {
                    // On odd rows this is the lower half of the block above.
                    if by % 2 == 0 {
                        self.scaled_block(&mut bits, cur, x, y)?;
                    }
                    bx += 2;
                    continue;
                }
                match kind {
                    block::SKIP => put8(cur, x, y, &fetch8(prev, y * prev.width + x)),
                    block::MOTION => {
                        let src = self.motion_source(prev, x, y)?;
                        put8(cur, x, y, &fetch8(prev, src));
                    }
                    block::RUN => {
                        let mut px = [0u8; 64];
                        self.run_block(&mut bits, &mut px)?;
                        put8(cur, x, y, &px);
                    }
                    block::RESIDUE => {
                        let src = self.motion_source(prev, x, y)?;
                        let mut px = fetch8(prev, src);
                        let budget = bits.read(7) as i32;
                        let mut residue = [0i8; 64];
                        read_residue(&mut bits, &mut residue, budget);
                        for (i, &r) in residue.iter().enumerate() {
                            let p = &mut px[SCAN[i] as usize];
                            *p = p.wrapping_add(r as u8);
                        }
                        put8(cur, x, y, &px);
                    }
                    block::INTRA => {
                        let px = self.intra_block(&mut bits);
                        put8(cur, x, y, &px);
                    }
                    block::FILL => {
                        let c = self.color();
                        fill(cur, x, y, 8, c);
                    }
                    block::INTER => {
                        let src = self.motion_source(prev, x, y)?;
                        let mut px = fetch8(prev, src);
                        let mut coefs = [0i16; 64];
                        coefs[0] = self.next(INTER_DC);
                        let q = read_dct(&mut bits, &mut coefs);
                        let diff = idct(&coefs, &INTER_QUANT[q]);
                        for (p, d) in px.iter_mut().zip(diff) {
                            *p = p.wrapping_add(d);
                        }
                        put8(cur, x, y, &px);
                    }
                    block::PATTERN => {
                        let mut px = [0u8; 64];
                        self.pattern_block(&mut px);
                        put8(cur, x, y, &px);
                    }
                    block::RAW => {
                        let mut px = [0u8; 64];
                        self.raw_block(&mut px);
                        put8(cur, x, y, &px);
                    }
                    _ => return Err("invalid block type"),
                }
                bx += 1;
            }
        }
        bits.align32();
        Ok(bits.position() / 8)
    }
}

/// Decodes whole frames.
pub(crate) struct VideoDecoder {
    planes: PlaneDecoder,
    revision: u8,
    alpha: bool,
    grayscale: bool,
}

impl VideoDecoder {
    pub fn new(revision: u8, alpha: bool, grayscale: bool) -> Self {
        Self { planes: PlaneDecoder::default(), revision, alpha, grayscale }
    }

    /// Decodes a video packet into `cur`, predicting from `prev`.
    ///
    /// Like the reference decoder this is lenient with damaged packets: bits
    /// missing at the end read as zero, and a plane whose data would start
    /// past the end of the packet is not decoded (it keeps its old contents).
    pub fn decode(&mut self, packet: &[u8], cur: &mut Frame, prev: &Frame) -> DecodeResult<()> {
        let signed_colors = self.revision < b'i';
        // Section sizes count from the start of their 4-byte size field.
        let size_at = |pos: usize| {
            packet.get(pos..pos + 4).map_or(usize::MAX, |b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
        };
        let mut pos = 0;
        // The alpha plane is always preceded by its size.
        if let (true, Some(a), Some(pa)) = (self.alpha, cur.alpha.as_mut(), prev.alpha.as_ref()) {
            self.planes.decode(packet.get(4..).unwrap_or(&[]), a, pa, signed_colors)?;
            pos = size_at(0);
        }
        if pos >= packet.len() {
            return Ok(());
        }
        // From revision `i` the luma plane is preceded by its size as well;
        // older planes simply follow each other.
        let next = if self.revision >= b'i' {
            self.planes.decode(packet.get(pos + 4..).unwrap_or(&[]), &mut cur.y, &prev.y, signed_colors)?;
            pos.saturating_add(size_at(pos))
        } else {
            pos + self.planes.decode(&packet[pos..], &mut cur.y, &prev.y, signed_colors)?
        };
        if self.grayscale {
            return Ok(());
        }
        // The first chroma plane is Cr from revision `h` on, Cb before.
        let (first, second, prev_first, prev_second) = if self.revision >= b'h' {
            (&mut cur.v, &mut cur.u, &prev.v, &prev.u)
        } else {
            (&mut cur.u, &mut cur.v, &prev.u, &prev.v)
        };
        let Some(rest) = packet.get(next..).filter(|r| !r.is_empty()) else { return Ok(()) };
        let used = self.planes.decode(rest, first, prev_first, signed_colors)?;
        let Some(rest) = rest.get(used..).filter(|r| !r.is_empty()) else { return Ok(()) };
        self.planes.decode(rest, second, prev_second, signed_colors)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_books_are_complete_prefix_codes() {
        for book in 0..16 {
            // Kraft equality: the 16 code words exactly fill the code space.
            let kraft: u32 = TREE_LENS[book].iter().map(|&l| 128 >> l).sum();
            assert_eq!(kraft, 128, "book {book}");
            // Every 7-bit window decodes to a code word that matches it.
            for (w, &e) in BOOK_LOOKUP[book].iter().enumerate() {
                let (len, sym) = ((e >> 4) as usize, (e & 15) as usize);
                assert_eq!(len, TREE_LENS[book][sym] as usize);
                assert_eq!(w & ((1 << len) - 1), TREE_CODES[book][sym] as usize);
            }
        }
    }

    #[test]
    fn scan_and_patterns_are_permutations() {
        let check = |p: &[u8; 64]| {
            let mut seen = [false; 64];
            p.iter().for_each(|&i| seen[i as usize] = true);
            assert!(seen.iter().all(|&s| s));
        };
        check(&SCAN);
        RUN_PATTERNS.iter().for_each(check);
    }

    /// Packs (value, bit count) pairs LSB first.
    fn pack(fields: &[(u32, u32)]) -> Vec<u8> {
        let mut out = vec![0u8; fields.iter().map(|f| f.1 as usize).sum::<usize>() / 8 + 8];
        let mut pos = 0;
        for &(v, n) in fields {
            for i in 0..n {
                if v >> i & 1 != 0 {
                    out[pos / 8] |= 1 << (pos % 8);
                }
                pos += 1;
            }
        }
        out
    }

    #[test]
    fn codebook_value_orders() {
        // Book 3 with an explicit list naming 5 and 2.
        let data = pack(&[(3, 4), (1, 1), (1, 3), (5, 4), (2, 4)]);
        let cb = Codebook::read(&mut Bits::new(&data));
        assert_eq!(cb.values, [5, 2, 0, 1, 3, 4, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
        // One merge pass of single values: take the right value of each pair.
        let mut fields = vec![(7, 4), (0, 1), (0, 2)];
        fields.extend([(1, 1); 8]);
        let cb = Codebook::read(&mut Bits::new(&pack(&fields)));
        assert_eq!(cb.values, [1, 0, 3, 2, 5, 4, 7, 6, 9, 8, 11, 10, 13, 12, 15, 14]);
    }

    /// A plane bitstream made of fill blocks of one colour.
    fn fill_plane(width: usize, height: usize, color: u8) -> Vec<u8> {
        let bw = width / 8;
        let count_bits = |max: usize| usize::BITS - max.leading_zeros();
        let mut fields = vec![(0, 4); 23]; // all code books: book 0, identity order
        for row in 0..height / 8 {
            // Block types: one chunk per row, all `FILL`.
            fields.extend([(bw as u32, count_bits(bw + 511)), (1, 1), (block::FILL as u32, 4)]);
            if row == 0 {
                fields.push((0, count_bits(width / 16 + 511))); // no sub-types
            }
            // Colours: a repeated value, high nibble first.
            let color = color as u32;
            fields.extend([(bw as u32, count_bits(bw * 64 + 511)), (1, 1), (color >> 4, 4), (color & 15, 4)]);
            if row == 0 {
                // Patterns, motion, DC and runs end at once.
                for max in [bw * 8 + 511, bw + 511, bw + 511, bw + 511, bw + 511, bw * 48 + 511] {
                    fields.push((0, count_bits(max)));
                }
            }
        }
        let mut data = pack(&fields);
        let used = fields.iter().map(|f| f.1 as usize).sum::<usize>().div_ceil(32) * 4;
        data.resize(used, 0);
        data
    }

    #[test]
    fn frame_layout_with_alpha() {
        let mut packet = Vec::new();
        for (plane, sized) in [(fill_plane(24, 16, 200), true), (fill_plane(24, 16, 100), true)] {
            if sized {
                packet.extend_from_slice(&(plane.len() as u32 + 4).to_le_bytes());
            }
            packet.extend_from_slice(&plane);
        }
        // Chroma of a 24x16 frame: 12x8, stored as 16x8. Cr comes first.
        packet.extend_from_slice(&fill_plane(16, 8, 30));
        packet.extend_from_slice(&fill_plane(16, 8, 220));
        let prev = Frame::new(24, 16, true);
        let mut cur = Frame::new(24, 16, true);
        VideoDecoder::new(b'i', true, false).decode(&packet, &mut cur, &prev).unwrap();
        assert!(cur.alpha.as_ref().unwrap().data.iter().all(|&a| a == 200));
        assert!(cur.y.data.iter().all(|&y| y == 100));
        assert!(cur.v.data.iter().all(|&v| v == 30));
        assert!(cur.u.data.iter().all(|&u| u == 220));
    }

    #[test]
    fn dc_only_idct_is_flat() {
        let mut coefs = [0i16; 64];
        coefs[0] = 100 * 8;
        let px = idct(&coefs, &INTRA_QUANT[0]);
        assert!(px.iter().all(|&p| p == 100));
    }

    #[test]
    fn dct_coefficients_follow_scan_order() {
        // Four bit planes; at the top plane coefficient 1 becomes significant
        // with magnitude 8 + 5 and a minus sign. Nothing else is significant.
        let mut fields = vec![(4, 4)];
        // Plane 3: list is [Twenty(4), Twenty(24), Twenty(44), Single(1), Single(2), Single(3)].
        fields.extend([(0, 1), (0, 1), (0, 1), (1, 1), (5, 3), (1, 1), (0, 1), (0, 1)]);
        // Planes 2..0: five pending entries, nothing significant.
        fields.extend([(0, 1); 15]);
        fields.push((9, 4));
        let data = pack(&fields);
        let mut bits = Bits::new(&data);
        let mut coefs = [0i16; 64];
        assert_eq!(read_dct(&mut bits, &mut coefs), 9);
        assert_eq!(coefs[SCAN[1] as usize], -13);
        assert_eq!(coefs.iter().filter(|&&c| c != 0).count(), 1);
        assert_eq!(bits.position(), 4 + 10 + 15 + 4);
    }
}
