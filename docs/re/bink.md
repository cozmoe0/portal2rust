# Bink 1 video

Facts about RAD Game Tools' Bink 1 format (`.bik`) as Portal 2 uses it,
gathered from the public format descriptions on MultimediaWiki ("Bink
Container", "Bink Video", "Bink Audio") and from the installed
`bin/binkw32.dll` (IDA). Written in our own words; no code is copied. The
decoder is `crates/bink`.

## Installed files

- `portal2/media` holds 55 files with 26,051 frames between them. All are
  revision `BIKi` and all have video flags 0: no alpha plane, no grayscale,
  no display scaling.
- Sizes: 1280x720 (27 files: menu backgrounds `menu_act01..05`, cinematics,
  attract loops, `valve.bik`), 640x400 (13, in-world screens), 960x540 (7,
  co-op "behind the scenes" screens), 480x640 (4, portrait screens), 640x360
  (3), 512x256 (`aperture_logo`).
- Frame rates (numerator/denominator): 5000000/208333 (24), 1199/50 (23.98),
  2997/125 (23.976), 18/1, 10000000/333667 (29.97), 24/1.
- 17 files carry audio, all Bink Audio, DCT variant, 16-bit:
  - one stereo 48 kHz track: `aperture_appear_horiz`, `turret_colours_type`,
    `turret_dropin` (silent), `valve_ps3_legals`;
  - one stereo 44.1 kHz track: `attract_boots`, `attract_bot_trust`,
    `attract_panels`, `attract_turrets`, `e1912_end`, `valve`;
  - two stereo 44.1 kHz tracks: `valve_xbox_legals`;
  - four 44.1 kHz tracks (stereo, mono, mono, stereo): `attract01`,
    `attract02`, `coop_intro`, `coop_outro`, `sp_30_a4_finale5`,
    `sp_ending_callback`.
- `startupvids.txt` lists `media/valve.bik`; `attractvids.txt` lists
  `attract01` and `attract02`.
- `valve_portal2_fp.bik` is `coop_black_screen.bik` (one black 640x360 frame)
  with an ASCII hex string written over part of its chroma data and 259 extra
  bytes appended after the last frame. Its second chroma plane runs past the
  end of the packet, and the DLL then decodes whatever memory follows, so its
  output for that plane differs from run to run. Nothing in the game DLLs
  names the file.

## Container

All fields little-endian.

| offset | size | field |
| --- | --- | --- |
| 0 | 3 | `BIK` |
| 3 | 1 | revision letter |
| 4 | 4 | file size minus 8 |
| 8 | 4 | frame count |
| 12 | 4 | largest frame in bytes |
| 16 | 4 | frame index length (number of offsets minus one) |
| 20 | 4 | width |
| 24 | 4 | height |
| 28 | 4 | frame rate numerator |
| 32 | 4 | frame rate denominator |
| 36 | 4 | video flags |
| 40 | 4 | audio track count |

- The DLL accepts revisions `f`, `g`, `h` and `i` only.
- It rejects a header with no frames, an index length below the frame count
  or above 1,000,000, a largest frame bigger than the size field, more than
  256 audio tracks, a size field below `4 * index length * (tracks + 1)`,
  width or height above 32767, or a zero frame rate term.
- Video flags: `0x00100000` alpha plane, `0x00020000` grayscale (no chroma
  planes are coded), bits 28..30 a display scaling mode (1 and 2 double the
  height, 3 the width, 4 and 5 both; the frame is still decoded at the coded
  size).
- After the fixed header come three arrays with one 32-bit entry per audio
  track:
  1. the largest decoded audio size of one frame in bytes
     (`BinkGetTrackMaxSize` returns it unchanged);
  2. sample rate in the low 16 bits, audio flags in the high 16 bits:
     `0x8000` Bink Audio with the FFT transform, `0x4000` 16-bit samples,
     `0x2000` stereo, `0x1000` Bink Audio with the DCT transform (when both
     transform bits are set the FFT one wins; a track with neither cannot be
     opened);
  3. the track id.
- Then `index length + 1` frame offsets from the start of the file. Bit 0
  marks a keyframe; the offset is the value with bit 0 cleared. Frame `n`
  occupies `offset[n] .. offset[n + 1]`.
- A frame holds, for every audio track in order, a 32-bit byte size and that
  many bytes of audio packet (possibly zero), then the video packet up to the
  end of the frame.

## Video packet

- Planes are coded one after another: the alpha plane if the file has one,
  luma, then the two chroma planes. From revision `h` the first chroma plane
  is Cr; for `f` and `g` it is Cb (the DLL keeps decoding into the same
  buffers and swaps the two chroma planes when converting to RGB).
- The alpha plane is always preceded by a 32-bit size counted from the start
  of that size field; the next plane starts that many bytes after it.
- From revision `i` the luma plane is preceded by a size of the same kind,
  which gives the start of the chroma data. Revisions `f` to `h` have no
  sizes; there the planes simply follow each other.
- Every plane's data ends on a 32-bit boundary.
- Plane sizes are padded to whole 8x8 blocks: luma `ceil8(w) x ceil8(h)`,
  chroma `ceil8(ceil(w / 2)) x ceil8(ceil(h / 2))`. The DLL decodes into
  buffers of exactly these sizes (rows padded to 16 bytes for chroma).
- The DLL keeps two frame buffers and alternates between them; the one not
  being written is the reference for skip and motion blocks. It never reports
  an error for damaged video data: a plane whose start lies outside the frame
  is skipped (the buffer keeps what it had), and reads past the end of the
  data take whatever bytes follow. Our decoder reads zeros instead.

## Plane bit stream

- Fields are packed into little-endian 32-bit words and read from the least
  significant bit upwards.
- Each plane starts with code book headers, in this order: block types,
  block sub-types, 16 books for the high nibble of colours, the colour book,
  patterns, motion X, motion Y, runs.
- There are 16 static prefix codes of 16 symbols each, with code lengths up
  to 7 bits; code words are stored first bit lowest. Code 0 is plain 4-bit
  values. A header is a 4-bit code index; for codes other than 0 it is
  followed by the order in which the 16 code words map to values:
  - flag bit 1: a 3-bit count `c`, then `c + 1` explicit 4-bit values; the
    values not named follow in ascending order;
  - flag bit 0: a 2-bit count `p`; starting from the identity order, `p + 1`
    passes merge neighbouring runs of 1, 2, 4 and 8 values, one bit per
    output value choosing the left (0) or right (1) run while both have
    values left.
- Per-block values come from nine bundles: block types, sub-types (for
  16x16 blocks), colours, patterns, motion X, motion Y, intra DC, inter DC
  and runs. At the start of every block row each bundle whose previous chunk
  is used up reads a new chunk, in the order just listed. A chunk begins with
  its value count in just enough bits to hold
  - `bw + 511` for block types, motion and DC values (`bw` = blocks per row),
  - `width / 16 + 511` for sub-types,
  - `64 * bw + 511` for colours,
  - `8 * bw + 511` for patterns,
  - `48 * bw + 511` for runs.

  A count of 0 ends that bundle for the rest of the plane.
- Chunk contents:
  - block types and sub-types: a bit set means one 4-bit value repeated;
    otherwise code book symbols, where 0..11 are values and 12..15 repeat the
    previous value 4, 8, 12 or 32 times (the previous value starts at 0 for
    every chunk);
  - colours: a bit set means one value repeated. A value is a high nibble
    from the high-nibble book selected by the previous high nibble (0 at the
    start of the plane, kept across chunks), then a low nibble from the colour
    book. Revisions before `i` store colours as sign and magnitude around
    128;
  - patterns: two symbols per value, low nibble first;
  - motion: a bit set means one 4-bit magnitude repeated; otherwise one
    symbol per value; a non-zero magnitude is followed by a sign bit
    (1 = negative);
  - DC: a start value (11 bits for intra, 10 bits and a sign bit for inter),
    then groups of up to eight deltas sharing a 4-bit width (each delta a
    magnitude and, when non-zero, a sign bit; width 0 repeats the value).
    Values are 16-bit; our decoder rejects a sum outside that range;
  - runs: a bit set means one 4-bit value repeated; otherwise symbols.

## Blocks

| type | meaning |
| --- | --- |
| 0 | skip: copy the co-located block of the reference frame |
| 1 | scaled: a 16x16 block coded at half resolution, kind from the sub-type bundle |
| 2 | motion: copy the reference block displaced by (motion X, motion Y) |
| 3 | run: a 4-bit pattern index, then runs of colours along that pixel order |
| 4 | residue: motion copy plus a sparse residue |
| 5 | intra: DCT |
| 6 | fill: one colour |
| 7 | inter: motion copy plus a DCT coded difference |
| 8 | pattern: two colours and eight pattern bytes, one bit per pixel (bit 0 = left) |
| 9 | raw: 64 colours |

- A scaled block on an even block row covers its block, the one to its right
  and the two below; it may be a run, intra, fill, pattern or raw block,
  decoded at 8x8 and doubled. The same type on an odd row stands for the
  lower half of such a block and skips two block columns.
- Motion vectors address the reference plane linearly (row stride times Y
  plus X), so a vector can reach past a side edge into the next row.
- Run blocks: each run is `run value + 1` pixels along the chosen order,
  preceded by a bit: 1 fills the run with one colour, 0 takes one colour per
  pixel. Runs stop once 63 or more pixels are covered; if exactly 63 are, the
  last pixel takes one more colour.
- DCT blocks take their DC from the bundle; the other 63 coefficients follow
  in Bink's own scan order. A 4-bit count gives the number of bit planes;
  from the top plane down, a work list is visited in which every pending
  entry reads one significance bit:
  - the initial entries are three groups of twenty coefficients (starting at
    scan indices 4, 24 and 44) and single coefficients 1, 2 and 3;
  - a significant group of twenty codes its first four coefficients now and
    becomes a group of sixteen; a significant group of sixteen splits into
    four groups of four, the first in its place and three appended;
  - a significant group of four codes its four coefficients: per
    coefficient a bit 1 defers it (it is put at the front of the list, so
    the next plane visits it first), a bit 0 codes it now;
  - coding a coefficient reads `plane` bits below a leading 1 and a sign
    bit.

  A 4-bit quantiser index follows the coefficients.
- Dequantisation multiplies each coefficient by a table entry and shifts
  right by 11. The DLL keeps 16 intra and 16 inter matrices in raster order.
- The inverse transform is a separable integer 8x8 IDCT, columns first,
  using the constants 2896, 2217, 3784 and -5352 with right shifts by 11. A
  pixel is `(value + 127) >> 8`.
- All pixel arithmetic in the DLL wraps modulo 256: intra results are stored
  as the low byte, and inter differences and residues are added to the
  reference pixels with byte wrap-around. There is no clamping.
- Residue blocks read a 7-bit budget and a 3-bit top level. For each level
  from the top mask (`1 << top`) down to 1, every coefficient that is already
  non-zero reads a bit and grows by the mask away from zero, then a pass over
  the same kind of work list as DCT blocks (starting with groups of twenty at
  4, 24 and 44 and a group of four at 0) introduces new coefficients of plus
  or minus the mask. The DLL keeps the values in signed bytes, so the
  direction of growth follows the sign of the stored byte. Decoding stops
  after `budget + 1` coefficient updates. The residue is added to the pixels
  in scan order.

## Audio (DCT variant)

- An audio packet starts with the number of bytes it decodes to (16-bit
  samples, channels interleaved). Transform blocks follow, each starting on
  a 32-bit boundary; the decoder decodes blocks until it has that many
  samples and drops the rest of the last block.
- The transform length `N` per channel is 512 below 22050 Hz, 1024 below
  44100 Hz and 2048 otherwise. The last `N / 16` samples of a block overlap
  the start of the next one, so a block yields `N - N / 16` samples per
  channel.
- Bands: with `nyquist = ceil(rate / 2)`, the band count is the position of
  the first band edge at or above `nyquist`, plus one, at most 25 (edges 100,
  200, 300, 400, 510, 630, 770, 920, 1080, 1270, 1480, 1720, 2000, 2320,
  2700, 3150, 3700, 4400, 5300, 6400, 7700, 9500, 12000 and 15500 Hz). Band
  `i` starts at coefficient `2 * max(1, (N / 2) * edge[i - 1] / nyquist)`
  (band 0 at 2) and the last band ends at `N`.
- A block skips 2 bits, then codes each channel in turn:
  - two floats (coefficients 0 and 1), each a 5-bit exponent `e`, a 23-bit
    mantissa `m` and a sign bit: `m * 2^(e - 23)`;
  - one 8-bit quantiser index per band (indices above 95 are clamped), the
    step being `10^(0.0664 * index)` (the DLL has these 96 steps as a float
    table);
  - from coefficient 2 to `N`: a bit choosing a run of 8 coefficients (bit
    0) or of `8 * table[4-bit index]` with table 2, 3, 4, 5, 6, 8, 9, 10, 11,
    12, 13, 14, 15, 16, 32, 64; then a 4-bit width. Width 0 makes the run
    zero; otherwise each coefficient is a `width`-bit magnitude and, when
    non-zero, a sign bit, scaled by the step of the band it falls in.
- Transform: `x[k] = sum over j < N of c[j] * cos(pi * j * (k + 1/2) / N)`
  (the DLL uses a float FFT-based DCT of exactly this form), then
  `x * 2 / sqrt(N)`.
- Conversion to 16 bits: with SSE (any current CPU) the DLL rounds to the
  nearest value, ties to even, and saturates; its non-SIMD fallback
  truncates instead.
- The first `count = N / 16 * channels` interleaved samples of each block
  are blended with the saved end of the previous block in 16-bit integer
  arithmetic: sample `i` becomes `prev + ((i * (cur - prev)) >> log2(count))`,
  the difference taken as a 16-bit wrapping subtraction and the result
  saturated. The first block after opening a track is not blended. The
  block's last `count` samples are kept for the next block.
- No installed file uses the FFT variant; `crates/bink` returns an error for
  it.

## Constant tables in `binkw32.dll`

Image base `0x18000000`, all in the `BINKCONS` and `BINKDATA` sections.

| address | contents |
| --- | --- |
| `0x1802B500` | code lengths, 16 x 16 bytes |
| `0x1802B600` | code words, 16 x 16 bytes |
| `0x1802B700` | DCT / residue scan order, 64 bytes |
| `0x1802B740` | run block pixel orders, 16 x 64 bytes |
| `0x1802BB40` | intra dequantisation, 16 x 64 `u32`, raster order |
| `0x1802CB40` | inter dequantisation, 16 x 64 `u32`, raster order |
| `0x1802FBD4` | block type runs 4, 8, 12, 32 |
| `0x18031294` | audio run table, 16 bytes |
| `0x180312AC` | audio band edges, 24 `u32` |
| `0x18031310` | audio quantiser steps, 96 floats |

Further dequantisation tables follow the inter ones at `0x1802DB40`; IDA
finds no reference to them.

## Verification

- The installed `binkw32.dll` was driven from a 32-bit host through its
  exported API (`BinkOpen`, `BinkDoFrame`, `BinkNextFrame`, frame buffers read
  through the `BINK` structure, audio through `BinkOpenTrack` and
  `BinkGetTrackData`) to record hashes of every decoded plane of every frame
  of all 55 files. `crates/bink` reproduces all frames of 54 files bit for
  bit; for `valve_portal2_fp.bik` it matches the luma and Cr planes.
- Decoded audio of four tracks (`valve`, `attract01` track 1, `e1912_end`,
  `turret_dropin`) has the same length as the DLL's; 99.99 % of the samples
  are identical and none differs by more than 1 (float rounding in the
  transform).
- No installed file has an alpha plane, so the alpha path is only covered by
  a synthetic test.
