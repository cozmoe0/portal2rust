//! Decoded frames: 8-bit YUV 4:2:0 planes and an RGBA conversion.

/// One 8-bit image plane. Its size is the coded size, rounded up to whole
/// 8x8 blocks; rows are stored without padding (`stride == width`).
#[derive(Clone, Debug)]
pub struct Plane {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Plane {
    pub(crate) fn new(width: usize, height: usize, fill: u8) -> Self {
        let (width, height) = (width.next_multiple_of(8), height.next_multiple_of(8));
        Self { width, height, data: vec![fill; width * height] }
    }

    pub fn row(&self, y: usize) -> &[u8] {
        &self.data[y * self.width..(y + 1) * self.width]
    }
}

/// A decoded video frame.
///
/// `y` and `alpha` cover the full frame, `u` (Cb) and `v` (Cr) half of it in
/// each direction. All planes are padded to multiples of 8 pixels; only the
/// top-left `width x height` (respectively half of it, rounded up) is
/// visible. Samples use the BT.601 "studio" range (luma 16..=235); alpha is
/// full range.
#[derive(Clone, Debug)]
pub struct Frame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) index: u32,
    pub(crate) keyframe: bool,
    pub y: Plane,
    pub u: Plane,
    pub v: Plane,
    pub alpha: Option<Plane>,
}

impl Frame {
    pub(crate) fn new(width: u32, height: u32, alpha: bool) -> Self {
        let (w, h) = (width as usize, height as usize);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        Self {
            width,
            height,
            index: 0,
            keyframe: false,
            y: Plane::new(w, h, 0),
            u: Plane::new(cw, ch, 0),
            v: Plane::new(cw, ch, 0),
            alpha: alpha.then(|| Plane::new(w, h, 0)),
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Index of this frame in the file.
    pub fn index(&self) -> u32 {
        self.index
    }

    pub fn is_keyframe(&self) -> bool {
        self.keyframe
    }

    /// Converts the visible area to tightly packed RGBA8 (BT.601, studio
    /// range). Alpha comes from the alpha plane, or is opaque without one.
    ///
    /// # Panics
    ///
    /// If `out` is shorter than `width * height * 4` bytes.
    pub fn to_rgba(&self, out: &mut [u8]) {
        let (w, h) = (self.width as usize, self.height as usize);
        assert!(out.len() >= w * h * 4, "RGBA buffer too small for a {w}x{h} frame");
        for (y, dst) in out.chunks_exact_mut(w * 4).take(h).enumerate() {
            let luma = &self.y.row(y)[..w];
            let cb = self.u.row(y / 2);
            let cr = self.v.row(y / 2);
            let pixels = dst.as_chunks_mut::<4>().0;
            // Two pixels share each chroma sample: its terms are worked out
            // once per pair (the same arithmetic as `yuv_to_rgb`).
            for (pair, (px, luma)) in pixels.chunks_mut(2).zip(luma.chunks(2)).enumerate() {
                let d = cb[pair] as i32 - 128;
                let e = cr[pair] as i32 - 128;
                let (r, g, b) = (104597 * e + 32768, -25675 * d - 53279 * e + 32768, 132201 * d + 32768);
                for (p, &l) in px.iter_mut().zip(luma) {
                    let c = (l as i32 - 16) * 76309;
                    *p = [
                        ((c + r) >> 16).clamp(0, 255) as u8,
                        ((c + g) >> 16).clamp(0, 255) as u8,
                        ((c + b) >> 16).clamp(0, 255) as u8,
                        255,
                    ];
                }
            }
            if let Some(alpha) = &self.alpha {
                for (p, &a) in pixels.iter_mut().zip(&alpha.row(y)[..w]) {
                    p[3] = a;
                }
            }
        }
    }
}

/// BT.601 studio-range YCbCr to RGB in 16.16 fixed point.
#[inline]
pub fn yuv_to_rgb(y: u8, cb: u8, cr: u8) -> [u8; 3] {
    let c = (y as i32 - 16) * 76309;
    let d = cb as i32 - 128;
    let e = cr as i32 - 128;
    let clamp = |v: i32| ((v + 32768) >> 16).clamp(0, 255) as u8;
    [clamp(c + 104597 * e), clamp(c - 25675 * d - 53279 * e), clamp(c + 132201 * d)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_colours() {
        assert_eq!(yuv_to_rgb(16, 128, 128), [0, 0, 0]);
        assert_eq!(yuv_to_rgb(235, 128, 128), [255, 255, 255]);
        // BT.601 primaries, which 8-bit YCbCr only reaches to within one step.
        let near = |a: [u8; 3], b: [u8; 3]| a.iter().zip(b).all(|(&x, y)| x.abs_diff(y) <= 1);
        assert!(near(yuv_to_rgb(81, 90, 240), [255, 0, 0]));
        assert!(near(yuv_to_rgb(145, 54, 34), [0, 255, 0]));
        assert!(near(yuv_to_rgb(41, 240, 110), [0, 0, 255]));
    }

    #[test]
    fn rgba_layout() {
        let mut f = Frame::new(3, 2, true);
        assert_eq!((f.y.width, f.y.height, f.u.width, f.u.height), (8, 8, 8, 8));
        f.y.data.fill(235);
        f.u.data.fill(128);
        f.v.data.fill(128);
        f.alpha.as_mut().unwrap().data[1] = 7;
        let mut out = vec![0; 3 * 2 * 4];
        f.to_rgba(&mut out);
        assert_eq!(&out[..8], &[255, 255, 255, 0, 255, 255, 255, 7]);
    }
}
