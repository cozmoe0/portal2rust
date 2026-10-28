# The 2D interface (VGUI)

How the original draws its interface, from the installed `vgui2.dll`,
`vguimatsurface.dll`, `shaderapidx9.dll` and `client.dll` (checked in IDA)
and screenshots of the running game, and how `crates/portal2/src/vgui` does
it.

## Blending

The interface blends in linear light. Colours and texels are gamma values
that are converted to linear, blended into the sRGB back buffer and encoded
again: the main menu's focus bar (white at alpha 15) over a background of 11
comes out at 70, which linear blending predicts exactly (gamma blending
would give 26). `render.rs` therefore draws the batches straight into the
linear HDR view after the scene, decoding vertex colours and texels from
sRGB in the shader. Blend modes: alpha (`SrcAlpha, InvSrcAlpha`), additive
(`SrcAlpha, One`, for `$additive` materials and additive fonts) and modulate
(`DstColor, Zero`, for modulating screen fades). VGUI textures here have
`NOMIP` and are drawn from mip 0 with bilinear filtering; `$alphatest`
texels under the reference are dropped.

## Conditionals and proportional units

- `shaderapidx9.dll` sets the layout symbols on every video mode change:
  `WIN32WIDE` when width / height >= 1.5999, `WIN32HIDEF` from 720 lines
  (`WIN32LODEF` below), and the console/anamorphic ones false.
- Proportional values scale by screen height / 480
  (`GetProportionalScaledValue`: `(int)(tall / 480.0 * value)`; the base is
  640x480).
- `Panel::ApplySettings`: `xpos`/`ypos` may start with `r` (screen size
  minus the value), `c` (half the screen plus the value) or `%` (percent of
  the screen); `wide`/`tall` with `f` (screen size minus the value) or `%`.
  Alignment is against the screen unless `proportionalToParent` is set.

## Schemes and fonts

- `CScheme::ReloadFontGlyphs` picks, for each font, the first numbered entry
  whose `yres` range holds the screen height (entries without `yres` always
  match). Proportional fonts with no range (or one going past 4000) scale
  `tall`, `blur` and `scanlines`; `tall` is capped at 127. Flags: italic 1,
  underline 2, strikeout 4, symbol 8, antialias 0x10, blur 0x20, rotary 0x40,
  dropshadow 0x80, additive 0x100, outline 0x200, custom 0x400, bitmap 0x800.
- `.vfont` files are TrueType with every byte XORed by a running key (the
  previous stored byte plus 167) and the salt (some bytes and their count,
  the count byte included) before `VFONT1` at the end.
- `CWin32Font` (on Windows NT) creates the font with `CreateFontA(tall, 0,
  0, 0, weight, italic, underline, strikeout, SYMBOL_CHARSET for symbol
  fonts else ANSI, ..., ANTIALIASED_QUALITY or NONANTIALIASED_QUALITY)`; the
  cell height is `tmHeight` plus 1 for a drop shadow and 2 for an outline.
- Widths come from `GetCharABCWidthsW` (then `A`, then the text extent);
  `a` loses blur + outline, `b` gains drop shadow + 2 x (blur + outline),
  `c` loses drop shadow + blur + outline, so the advance stays `a + b + c`.
  There is no kerning.
- Antialiased glyphs (up to 0xFF, or any for custom fonts) take
  `GetGlyphOutlineA(GGO_GRAY8_BITMAP)`: white where the level is non-zero,
  alpha `level / 64`, placed at `ascent - origin.y` and offset by blur +
  outline (black boxes 2 or more wider than `b` are cropped evenly). Other
  glyphs are drawn white on black with `ExtTextOutW` into a DIB and take
  alpha `0.55 G + 0.34 B + 0.11 R`.
- Effects, in order: drop shadow (empty pixels take the alpha of the pixel
  one up and left, in black), outline (empty pixels next to a fully
  coloured one become opaque black), Gaussian blur of the first channel
  (sigma = radius x 0.683, with pi = 3.14 and e = 2.7), scan lines (every
  row but each Nth at 70%) and rotary (a grey middle row).
- `Panel::DrawBox` (used by close captions): a filled box whose corners are
  the 8x8 `vgui/hud/800corner1-4` textures at their own size.

## HUD

- `scripts/hudlayout.res` places the elements.
- Close captions (`CHudCloseCaption`): fonts `CloseCaption_Normal/Italic/
  Bold/BoldItalic` of `basemodui_scheme`; line height the normal font's
  height (at least 6); lines wrap at the panel width less 24 pixels, a
  character that does not fit moving the word since the last space to a new
  line (the space dropped) or starting a new line; the box is the text
  height plus 24, from the bottom of the panel, with text inset 12; height
  and alpha move towards new targets over `GrowTime`, each frame by the
  elapsed fraction; items over the available height are dropped oldest
  first down to `cc_minvisibleitems` (1); item alpha is 0 for
  `ItemHiddenTime`, fades in over `ItemFadeInTime` and out over
  `ItemFadeOutTime`; text is drawn 3 pixels in from the line start.
- `game_text` (`CHudMessage`): channel 2 uses `InGameChapterTitle`, channel 3
  `InGameChapterSubtitle` (proportional, `basemodui_scheme`). Lines are
  placed by `x`/`y` (-1 centres, negative counts from the far edge) and
  clamped on screen. Effects 0/1 fade the whole message in over `fadein`,
  hold, and out over `fadeout`; effect 2 types the characters out one per
  `fadein` seconds, each blending from `color2` to `color` over `fxtime`,
  and fades out after `fadein x length + holdtime`. Characters are drawn
  with alpha 255 less the message fade.
- Crosshair (`CHUDQuickInfo`): a one-pixel white centre dot and two
  brackets from `sprites/hud/portal_crosshairs` (`scripts/mod_textures.txt`
  rectangles, 44x64 at their own size), the first portal's 6 left and 10 up
  from the centre, the second's 6 right and 10 down; portal colours (64 160
  255 / 255 160 32) are brightened by `(1 - c/255) x 64 + c` (at least 5);
  empty brackets show at alpha 128, filled ones once the portal is placed.
