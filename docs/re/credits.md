# Ending credits

How the installed game rolls the single-player ending credits
(`env_portal_credits` `RollPortalOutroCredits`, fired by `sp_a5_credits`
at map spawn). Gathered from the installed `client.dll` (IDA, the
CHudPortalCredits HUD element), `scripts/credits.txt` and
`resource/basemodui_scheme.res`, and compared with screenshots of the game
(`re/shots/m11_credits.txt`; the HUD must stay on). Written in our own
words; no code is copied.

## Data

- `scripts/credits.txt`: `CreditsParams` (`scrolltime` 130, `separation`
  5, `cursorblinktime` 0.3, `scrollcreditsstart` 5.2, `color_lyrics`
  240 182 0 70, `color_credits` 0 0 0 192; code defaults 57, 5, 0.3, 6,
  ...), `OutroCreditsNames` (name, font), `OutroSongLyrics` (timed text,
  font). Names and lines starting with `#` are localisation tokens.
- Font `CreditsOutroText`: Consolas, tall 20, proportional.

## Layout

- Margins: 7.5% of the screen width and height, kept inside a 16:9 area
  (pillar or letter boxes add to them). Names never rise above
  20% of the boxed height (plus half the box).
- Lines are the font's height plus `separation` apart.
- Lyrics at the left margin (plus the width of the line so far for
  continued pieces), their row times the line step below the top margin,
  45 pixels lower on screens at least 720 tall. Drawn in `color_lyrics`.
- Names right-aligned at the right margin; the one being typed sits a line
  step above the bottom margin and each new one pushes the earlier ones a
  line up. Drawn in `color_credits` (dark over the orange background).

## Lyric lines

`[seconds]` is the time the piece types over; pieces follow one another.
Text after `>>>` only is used. `*text` keeps the next piece on the same
line, after this one. `^` is an empty line. A lone space is a pause (no
new line). `&` clears the screen: lines before it vanish once it starts,
and the next line is the top one again.

## Typing

Over its time a line shows `len × progress + 1` (clamped) characters: all
but the last of them, then the cursor `_` while it is on (it toggles
every `cursorblinktime`). A name being typed is padded with spaces to its
full length so its right alignment does not move. Names share
`scrolltime` in proportion to their length, starting at
`scrollcreditsstart`.

## Movies

From the start `media/sp_credits_bg.bik` loops behind everything (and the
song's captions go to the subtitle display,
`resource/cheaptitles/credits_song_captions.txt`). Four seconds after the
last name has typed, `media/sp_ending_callback.bik` plays once, then the
game disconnects to the menu.
