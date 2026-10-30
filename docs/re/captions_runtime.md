# Close captions on screen

How `crates/portal2_ui/src/captions.rs` shows captions, from the installed
data (`scripts/hudlayout.res`, `resource/basemodui_scheme.res`,
`resource/captioning_colors.txt`) and the caption format notes in
`audio_formats.md`.

## When a caption appears

- Captions show only with `closecaption 1`; `cc_subtitles 1` drops
  sound-effect captions (text with `<sfx>`). Defaults and the user's
  `config.cfg` both have `closecaption 0`, `cc_subtitles 0`,
  `cc_linger_time 1.0`, `cc_predisplay_time 0.25`, `cc_lang ""` (English).
- Scene speak events caption their line themselves, with the event's
  `cctoken` or else the sound name, and only for `cc_master` events. Other
  sounds started from a sound entry look the entry name up.
- A caption appears `cc_predisplay_time` after its sound starts (sound
  effects at once) and stays for the sound's length, or `<len:n>`, plus
  `cc_linger_time`. Text after `<delay:n>` appears n seconds later as its
  own item; `<norepeat:n>` drops repeats within n seconds. Text starting
  with `!!!` or `-->` is never shown.
- `<clr:r,g,b>` sets the colour (white by default); when the text then
  continues with `Name:`, the speaker's colour from
  `captioning_colors.txt` (`"glados" "<clr:163,193,173>"`, ...) replaces it.

## The panel

`HudCloseCaption` in `scripts/hudlayout.res`, in 640×480 units scaled by the
screen height: `xpos c-250` (centred), `ypos 276`, `wide 500`, `tall 136`,
`BgAlpha 128`, `GrowTime 0.25`, `ItemHiddenTime 0.2`, `ItemFadeInTime 0.15`,
`ItemFadeOutTime 0.3`. Newest lines sit at the bottom.

Fonts (`basemodui_scheme.res`): `CloseCaption_Normal`, `_Italic`, `_Bold`
and `_BoldItalic` are `UniversLTStd-Cn`, `tall 20`, weight 500 (900 bold),
antialiased. The scheme's `CustomFontFiles` load them from
`vgui/fonts/universc.vfont` (with `univercb`, `univercl` for bold and light).

## `.vfont`

A TrueType file with its bytes scrambled; the file ends with `VFONT1`,
before that a salt length byte and that many salt bytes. Each font byte is
XORed with a running key, and after each byte the key becomes the stored
byte plus 167 (mod 256). The starting key comes from the salt; because the
first plain byte of a TrueType font is always 0, the reader takes the first
stored byte as the key instead of deriving it
(`source_formats::vfont::decode`). All four installed vfonts decode to
valid TrueType tables.
