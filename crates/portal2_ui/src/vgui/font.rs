//! Fonts the way the game's `vguimatsurface.dll` builds them on Windows.
//!
//! A font is a GDI font created from the scheme's face name, height, weight
//! and flags (custom faces come from the decoded `.vfont` files, added to
//! the process with `AddFontMemResourceEx`). Each glyph becomes an RGBA
//! cell `b × height` pixels in size: antialiased fonts take GDI's 65-level
//! grey glyph bitmap (white, alpha = level / 64); other fonts and glyphs
//! GDI has no bitmap for are drawn with `ExtTextOutW` into a DIB and the
//! alpha is a weighted sum of its channels. The cell then gets the effects
//! the font asks for, in this order: drop shadow, outline, blur, scan
//! lines, rotary. See `docs/re/vgui.md`.

use std::collections::HashMap;

/// Scheme font flags (`ISurface::EFontFlags`).
pub mod flags {
    pub const ITALIC: u32 = 0x001;
    pub const UNDERLINE: u32 = 0x002;
    pub const STRIKEOUT: u32 = 0x004;
    pub const SYMBOL: u32 = 0x008;
    pub const ANTIALIAS: u32 = 0x010;
    pub const ROTARY: u32 = 0x040;
    pub const DROPSHADOW: u32 = 0x080;
    pub const ADDITIVE: u32 = 0x100;
    pub const OUTLINE: u32 = 0x200;
    pub const CUSTOM: u32 = 0x400;
    pub const BITMAP: u32 = 0x800;
}

/// Everything that identifies one rasterized font.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FontSpec {
    pub face: String,
    pub tall: i32,
    pub weight: i32,
    pub flags: u32,
    pub blur: i32,
    pub scanlines: i32,
}

impl FontSpec {
    pub fn additive(&self) -> bool {
        self.flags & flags::ADDITIVE != 0
    }
    fn dropshadow(&self) -> i32 {
        (self.flags & flags::DROPSHADOW != 0) as i32
    }
    fn outline(&self) -> i32 {
        (self.flags & flags::OUTLINE != 0) as i32
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Metrics {
    /// Cell height: GDI height plus drop shadow and twice the outline.
    pub height: i32,
    pub ascent: i32,
    pub max_char_width: i32,
}

/// A glyph's ABC widths with the effects' padding folded in, so that the
/// cell is drawn at `pen + a` and the pen advances by `a + b + c`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Abc {
    pub a: i32,
    pub b: i32,
    pub c: i32,
}

impl Abc {
    pub fn advance(&self) -> i32 {
        self.a + self.b + self.c
    }
}

/// Adds font bytes (TrueType) for the rest of the process; false when the
/// system rejected them.
pub fn register_font_data(data: Vec<u8>) -> bool {
    platform::register(data)
}

/// One created font: metrics, widths and glyph cells.
pub struct RasterFont {
    spec: FontSpec,
    inner: Option<platform::Font>,
    metrics: Metrics,
    abc: HashMap<char, Abc>,
}

impl RasterFont {
    pub fn new(spec: FontSpec) -> Self {
        let inner = platform::Font::create(&spec);
        let metrics = inner.as_ref().map(|f| f.metrics(&spec)).unwrap_or_default();
        if inner.is_none() {
            bevy::log::warn!("vgui: font '{}' {} could not be created", spec.face, spec.tall);
        }
        Self { spec, inner, metrics, abc: HashMap::new() }
    }

    pub fn spec(&self) -> &FontSpec {
        &self.spec
    }

    pub fn metrics(&self) -> Metrics {
        self.metrics
    }

    pub fn abc(&mut self, ch: char) -> Abc {
        if let Some(abc) = self.abc.get(&ch) {
            return *abc;
        }
        let raw = self.inner.as_ref().map(|f| f.abc(ch, &self.metrics)).unwrap_or((0, 0, 0));
        let (blur, outline, shadow) = (self.spec.blur, self.spec.outline(), self.spec.dropshadow());
        let abc = Abc { a: raw.0 - blur - outline, b: raw.1 + shadow + 2 * (blur + outline), c: raw.2 - shadow - blur - outline };
        self.abc.insert(ch, abc);
        abc
    }

    /// The glyph's RGBA cell, `abc(ch).b` wide and `metrics().height` tall.
    pub fn rasterize(&mut self, ch: char) -> (i32, i32, Vec<u8>) {
        let abc = self.abc(ch);
        let (w, h) = (abc.b.max(0), self.metrics.height.max(0));
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        if let Some(font) = &self.inner {
            font.draw(ch, abc, &self.spec, &self.metrics, w, h, &mut rgba);
        }
        apply_dropshadow(w, h, &mut rgba, self.spec.dropshadow());
        apply_outline(w, h, &mut rgba, self.spec.outline());
        apply_blur(w, h, &mut rgba, self.spec.blur);
        apply_scanlines(w, h, &mut rgba, self.spec.scanlines);
        apply_rotary(w, h, &mut rgba, self.spec.flags & flags::ROTARY != 0);
        (w, h, rgba)
    }
}

/// Where a pixel is empty, take the alpha of the pixel `offset` up and to
/// the left, in black.
fn apply_dropshadow(w: i32, h: i32, rgba: &mut [u8], offset: i32) {
    if offset == 0 || h - 1 < offset {
        return;
    }
    for y in (offset..h).rev() {
        for x in (offset..w).rev() {
            let i = ((y * w + x) * 4) as usize;
            if rgba[i + 3] == 0 {
                let src = (((y - offset) * w + x - offset) * 4) as usize;
                rgba[i] = 0;
                rgba[i + 1] = 0;
                rgba[i + 2] = 0;
                rgba[i + 3] = rgba[src + 3];
            }
        }
    }
}

/// Empty pixels next to a fully coloured one become opaque black.
fn apply_outline(w: i32, h: i32, rgba: &mut [u8], size: i32) {
    if size == 0 {
        return;
    }
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            if rgba[i + 3] != 0 {
                continue;
            }
            'search: for sx in -size..=size {
                for sy in -size..=size {
                    if sx == 0 && sy == 0 {
                        continue;
                    }
                    let (nx, ny) = (x + sx, y + sy);
                    if nx < 0 || nx >= w || ny < 0 || ny >= h {
                        continue;
                    }
                    let n = ((ny * w + nx) * 4) as usize;
                    if rgba[n] != 0 && rgba[n + 1] != 0 && rgba[n + 2] != 0 && rgba[n + 3] != 0 {
                        rgba[i] = 0;
                        rgba[i + 1] = 0;
                        rgba[i + 2] = 0;
                        rgba[i + 3] = 255;
                        break 'search;
                    }
                }
            }
        }
    }
}

/// Gaussian blur of the first colour channel into the alpha channel; the
/// colour becomes white. The game's kernel uses pi = 3.14 and e = 2.7.
fn apply_blur(w: i32, h: i32, rgba: &mut [u8], radius: i32) {
    if radius <= 0 || w <= 0 || h <= 0 {
        return;
    }
    let sigma = radius as f64 * 0.683;
    let scale = 1.0 / (sigma * 6.28 * sigma).sqrt();
    let kernel: Vec<f32> = (-radius..=radius).map(|x| (2.7f64.powf(-((x * x) as f64) / (sigma * 2.0 * sigma)) * scale) as f32).collect();
    let src = rgba.to_vec();
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0.0f32;
            for px in (x - radius).max(0)..=(x + radius).min(w - 1) {
                let kx = kernel[(px - x + radius) as usize];
                for py in (y - radius).max(0)..=(y + radius).min(h - 1) {
                    let ky = kernel[(py - y + radius) as usize];
                    sum += ky * kx * src[((py * w + px) * 4) as usize] as f32;
                }
            }
            let i = ((y * w + x) * 4) as usize;
            rgba[i] = 255;
            rgba[i + 1] = 255;
            rgba[i + 2] = 255;
            rgba[i + 3] = (sum as i32).min(255) as u8;
        }
    }
}

/// Every row but each `every`th is dimmed to 70%.
fn apply_scanlines(w: i32, h: i32, rgba: &mut [u8], every: i32) {
    if every < 2 {
        return;
    }
    for y in 0..h {
        if y % every == 0 {
            continue;
        }
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            for c in 0..3 {
                rgba[i + c] = (rgba[i + c] as f32 * 0.7) as u8;
            }
        }
    }
}

/// A grey line through the middle row.
fn apply_rotary(w: i32, h: i32, rgba: &mut [u8], on: bool) {
    if !on || h <= 0 {
        return;
    }
    let y = h / 2;
    for x in 0..w {
        let i = ((y * w + x) * 4) as usize;
        rgba[i..i + 4].copy_from_slice(&[127, 127, 127, 255]);
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::CString;
    use std::sync::Mutex;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Graphics::Gdi::*;

    use super::{Abc, FontSpec, Metrics, flags};

    static REGISTERED: Mutex<Vec<(usize, Vec<u8>)>> = Mutex::new(Vec::new());

    pub fn register(data: Vec<u8>) -> bool {
        let mut count = 0u32;
        // SAFETY: the bytes stay alive (kept in REGISTERED) for the process.
        let handle: HANDLE = unsafe { AddFontMemResourceEx(data.as_ptr().cast(), data.len() as u32, std::ptr::null(), &mut count) };
        if handle.is_null() {
            return false;
        }
        REGISTERED.lock().unwrap().push((handle as usize, data));
        true
    }

    /// GDI objects of one font: a memory DC with the font selected and a
    /// 32-bit top-down DIB the size of the widest glyph.
    pub struct Font {
        dc: HDC,
        font: HFONT,
        bitmap: HBITMAP,
        bits: *mut u8,
        bmp_wide: i32,
        bmp_tall: i32,
    }

    // SAFETY: the handles are only used through `&self` from one thread at a
    // time (the font cache lives behind a resource), and GDI memory DCs are
    // not tied to the creating thread.
    unsafe impl Send for Font {}
    unsafe impl Sync for Font {}

    impl Font {
        pub fn create(spec: &FontSpec) -> Option<Self> {
            let name = CString::new(spec.face.as_str()).ok()?;
            let charset = if spec.flags & flags::SYMBOL != 0 { SYMBOL_CHARSET } else { ANSI_CHARSET };
            let quality = if spec.flags & flags::ANTIALIAS != 0 { ANTIALIASED_QUALITY } else { NONANTIALIASED_QUALITY };
            // SAFETY: plain GDI object creation; every handle is checked.
            unsafe {
                let dc = CreateCompatibleDC(std::ptr::null_mut());
                let font = CreateFontA(
                    spec.tall,
                    0,
                    0,
                    0,
                    spec.weight,
                    (spec.flags & flags::ITALIC != 0) as u32,
                    (spec.flags & flags::UNDERLINE != 0) as u32,
                    (spec.flags & flags::STRIKEOUT != 0) as u32,
                    charset as u32,
                    OUT_DEFAULT_PRECIS as u32,
                    CLIP_DEFAULT_PRECIS as u32,
                    quality as u32,
                    (DEFAULT_PITCH | FF_DONTCARE) as u32,
                    name.as_ptr().cast(),
                );
                if font.is_null() {
                    DeleteDC(dc);
                    return None;
                }
                SetMapMode(dc, MM_TEXT);
                SelectObject(dc, font);
                SetTextAlign(dc, TA_UPDATECP);
                let mut tm: TEXTMETRICA = std::mem::zeroed();
                if GetTextMetricsA(dc, &mut tm) == 0 {
                    DeleteObject(font);
                    DeleteDC(dc);
                    return None;
                }
                let outline = (spec.flags & flags::OUTLINE != 0) as i32;
                let shadow = (spec.flags & flags::DROPSHADOW != 0) as i32;
                let bmp_wide = tm.tmMaxCharWidth + 2 * outline;
                let bmp_tall = tm.tmHeight + shadow + 2 * outline;
                let mut info: BITMAPINFO = std::mem::zeroed();
                info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                info.bmiHeader.biWidth = bmp_wide;
                info.bmiHeader.biHeight = -bmp_tall;
                info.bmiHeader.biPlanes = 1;
                info.bmiHeader.biBitCount = 32;
                info.bmiHeader.biCompression = BI_RGB;
                let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
                let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
                if bitmap.is_null() || bits.is_null() {
                    DeleteObject(font);
                    DeleteDC(dc);
                    return None;
                }
                SelectObject(dc, bitmap);
                Some(Self { dc, font, bitmap, bits: bits.cast(), bmp_wide, bmp_tall })
            }
        }

        pub fn metrics(&self, spec: &FontSpec) -> Metrics {
            // SAFETY: the DC is valid for the font's life.
            let mut tm: TEXTMETRICA = unsafe { std::mem::zeroed() };
            unsafe { GetTextMetricsA(self.dc, &mut tm) };
            let outline = (spec.flags & flags::OUTLINE != 0) as i32;
            let shadow = (spec.flags & flags::DROPSHADOW != 0) as i32;
            Metrics { height: tm.tmHeight + shadow + 2 * outline, ascent: tm.tmAscent, max_char_width: tm.tmMaxCharWidth }
        }

        /// Raw ABC widths from GDI, with the game's fallbacks.
        pub fn abc(&self, ch: char, metrics: &Metrics) -> (i32, i32, i32) {
            let code = ch as u32;
            let mut abc: ABC = ABC { abcA: 0, abcB: 0, abcC: 0 };
            // SAFETY: valid DC and output pointer.
            unsafe {
                if GetCharABCWidthsW(self.dc, code, code, &mut abc) != 0 || GetCharABCWidthsA(self.dc, code, code, &mut abc) != 0 {
                    return (abc.abcA, abc.abcB as i32, abc.abcC);
                }
                let mut buf = [0u16; 2];
                let utf16 = ch.encode_utf16(&mut buf);
                let mut mb = [0u8; 8];
                let n = windows_sys::Win32::Globalization::WideCharToMultiByte(
                    0,
                    0,
                    utf16.as_ptr(),
                    utf16.len() as i32,
                    mb.as_mut_ptr(),
                    6,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                );
                let mut size = windows_sys::Win32::Foundation::SIZE { cx: 0, cy: 0 };
                if n > 0 && GetTextExtentPoint32A(self.dc, mb.as_ptr(), n, &mut size) != 0 {
                    return (0, size.cx, 0);
                }
            }
            (0, metrics.max_char_width, 0)
        }

        #[allow(clippy::too_many_arguments)]
        pub fn draw(&self, ch: char, abc: Abc, spec: &FontSpec, metrics: &Metrics, w: i32, h: i32, rgba: &mut [u8]) {
            let code = ch as u32;
            let antialias = spec.flags & flags::ANTIALIAS != 0 && (code <= 0xff || spec.flags & flags::CUSTOM != 0);
            if antialias && self.draw_gray(code, abc, spec, metrics, w, h, rgba) {
                return;
            }
            self.draw_text_out(ch, abc, spec, metrics, w, h, rgba);
        }

        /// The 65-level grey bitmap from `GetGlyphOutlineA`; false when GDI
        /// has no bitmap for the glyph.
        #[allow(clippy::too_many_arguments)]
        fn draw_gray(&self, code: u32, abc: Abc, spec: &FontSpec, metrics: &Metrics, w: i32, h: i32, rgba: &mut [u8]) -> bool {
            let identity = MAT2 {
                eM11: FIXED { fract: 0, value: 1 },
                eM12: FIXED { fract: 0, value: 0 },
                eM21: FIXED { fract: 0, value: 0 },
                eM22: FIXED { fract: 0, value: 1 },
            };
            // SAFETY: valid DC; the buffer has the size GDI asked for.
            unsafe {
                SelectObject(self.dc, self.font);
                let mut gm: GLYPHMETRICS = std::mem::zeroed();
                let size = GetGlyphOutlineA(self.dc, code, GGO_GRAY8_BITMAP, &mut gm, 0, std::ptr::null_mut(), &identity) as i32;
                if size <= 0 {
                    return false;
                }
                let mut buf = vec![0u8; size as usize];
                GetGlyphOutlineA(self.dc, code, GGO_GRAY8_BITMAP, &mut gm, size as u32, buf.as_mut_ptr().cast(), &identity);
                let box_w = gm.gmBlackBoxX as i32;
                let pitch = (box_w + 3) / 4 * 4;
                let top = metrics.ascent - gm.gmptGlyphOrigin.y;
                // Boxes wider than the cell are cropped evenly on both sides.
                let crop = if box_w >= abc.b + 2 { (box_w - abc.b) >> 1 } else { 0 };
                let pad = spec.blur + (spec.flags & flags::OUTLINE != 0) as i32;
                for row in 0..gm.gmBlackBoxY as i32 {
                    let y = top + row;
                    for i in crop..box_w {
                        let x = i + pad - crop;
                        if x >= w || y >= h || x < 0 || y < 0 {
                            continue;
                        }
                        let level = buf[(row * pitch + i) as usize];
                        let o = ((y * w + x) * 4) as usize;
                        let alpha = if level != 0 { (level as f32 * 0.015625).min(1.0) } else { 0.0 };
                        // Tabs keep their coverage but no colour.
                        let c = if level != 0 && code != 9 { 255 } else { 0 };
                        rgba[o] = c;
                        rgba[o + 1] = c;
                        rgba[o + 2] = c;
                        rgba[o + 3] = (alpha * 255.0) as u8;
                    }
                }
            }
            true
        }

        /// White text on black with `ExtTextOutW`; alpha from the channels.
        #[allow(clippy::too_many_arguments)]
        fn draw_text_out(&self, ch: char, abc: Abc, spec: &FontSpec, metrics: &Metrics, w: i32, h: i32, rgba: &mut [u8]) {
            let code = ch as u32;
            let outline = (spec.flags & flags::OUTLINE != 0) as i32;
            let shadow = (spec.flags & flags::DROPSHADOW != 0) as i32;
            let underline = spec.flags & flags::UNDERLINE != 0;
            let Abc { a, b, c } = abc;
            let mut wide = if underline { a + b + c } else { b };
            let mut rows = metrics.height;
            let mut buf = [0u16; 2];
            let utf16 = ch.encode_utf16(&mut buf);
            // SAFETY: valid DC with the DIB selected; reads stay inside it.
            unsafe {
                SelectObject(self.dc, self.font);
                SetBkColor(self.dc, 0);
                SetTextColor(self.dc, 0x00ff_ffff);
                SetBkMode(self.dc, OPAQUE as i32);
                MoveToEx(self.dc, if underline { 0 } else { -a }, 0, std::ptr::null_mut());
                let rect = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: wide, bottom: rows };
                ExtTextOutW(self.dc, 0, 0, ETO_OPAQUE, &rect, std::ptr::null(), 0, std::ptr::null());
                ExtTextOutW(self.dc, 0, 0, 0, std::ptr::null(), utf16.as_ptr(), utf16.len() as u32, std::ptr::null());
                SetBkMode(self.dc, TRANSPARENT as i32);
                windows_sys::Win32::Graphics::Gdi::GdiFlush();
                wide = wide.min(self.bmp_wide);
                rows = rows.min(self.bmp_tall);
                let bits = std::slice::from_raw_parts(self.bits, (self.bmp_wide * self.bmp_tall * 4) as usize);
                for y in outline..rows - outline {
                    for x in outline..wide - outline - shadow {
                        if x >= w || y >= h {
                            continue;
                        }
                        let s = ((y * self.bmp_wide + x) * 4) as usize;
                        let (bl, gr, rd) = (bits[s], bits[s + 1], bits[s + 2]);
                        let o = ((y * w + x) * 4) as usize;
                        let (bl, gr, rd) = if code == 9 { (0, 0, 0) } else { (bl, gr, rd) };
                        rgba[o] = bl;
                        rgba[o + 1] = gr;
                        rgba[o + 2] = rd;
                        rgba[o + 3] = ((gr as f32 * 0.55000001 + bl as f32 * 0.34) + rd as f32 * 0.11) as i32 as u8;
                    }
                }
            }
            if shadow != 0 && metrics.height > 0 && metrics.height <= h {
                let row = ((metrics.height - 1) * w * 4) as usize;
                let n = (wide.min(w) * 4) as usize;
                rgba[row..row + n].fill(0);
            }
        }
    }

    impl Drop for Font {
        fn drop(&mut self) {
            // SAFETY: the handles were created by `create` and are freed once.
            unsafe {
                DeleteObject(self.bitmap);
                DeleteObject(self.font);
                DeleteDC(self.dc);
            }
        }
    }
}

#[cfg(not(windows))]
mod platform {
    //! Without GDI there is no rasterizer yet; text is measured as empty.
    use super::{Abc, FontSpec, Metrics};

    pub fn register(_data: Vec<u8>) -> bool {
        false
    }

    pub struct Font;

    impl Font {
        pub fn create(_spec: &FontSpec) -> Option<Self> {
            None
        }
        pub fn metrics(&self, _spec: &FontSpec) -> Metrics {
            Metrics::default()
        }
        pub fn abc(&self, _ch: char, _metrics: &Metrics) -> (i32, i32, i32) {
            (0, 0, 0)
        }
        #[allow(clippy::too_many_arguments)]
        pub fn draw(&self, _ch: char, _abc: Abc, _spec: &FontSpec, _metrics: &Metrics, _w: i32, _h: i32, _rgba: &mut [u8]) {}
    }
}
