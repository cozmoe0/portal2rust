# Menus (BaseModUI)

How the original's menus behave, from `client.dll` (IDA) and screenshots of
the running game (`re/tools/capture_ui.py`), and how
`crates/portal2/src/frontend/gameui` does it.

## Startup and background

- `media/startupvids.txt` lists the startup videos (`media/valve.bik`);
  `-novid` skips them, any key or click skips one.
- The main menu draws over `media/menu_act<N>.bik` (looping), or the still
  `vgui/backgrounds/background<N>` (`_widescreen` on wide screens) with
  `-nomenuvid`, and plays `music/mainmenu/portal2_background<N>.wav`.
- N: the chapter of the most recent save gives the act (the `sp_a<N>` of
  the chapter's first map, from the game's chapter table); without a save,
  `ui_lastact_played` (archived, 0-100) when under 5; `-background N`
  overrides; clamped to 1-5. Finishing the game (`cl_finale_completed`) moves
  act 4 on to 5.
- The logo is `vgui/portal2logo` at the scheme's `Logo.X/Y/Width/Height`.
- The live main menu layout is `resource/ui/basemodui/mainmenu_new.res`.

## Buttons and dialogs

- Hybrid buttons take `<Style>.*` scheme settings over `HybridButton.*`.
  The focused one gets a `CursorHeight` (25 units) bar from its top in
  `CursorColor`; text in `TextColor`/`FocusColor` (`DisabledColor` when
  disabled), upper case with `AllCaps`, at `TextInsetX/Y` from the button's
  corner. Bitmap buttons draw their icon (`bitmap_enabled`/`bitmap_focus`)
  in a square the button's height first.
- Dialog-style frames (`dialogstyle 1`) are sized in tiles of
  `Dialog.TileWidth/TileHeight` (50 units): the file's `wide`/`tall` count
  the body's tiles; the body is pinned `Dialog.PinFromLeft` from the left and
  `Dialog.PinFromBottom` above the bottom; a title sits in a tab one tile
  row above the body, as many tiles wide as the title text plus twice
  `Dialog.TitleOffsetX`, drawn in `Dialog.TitleFont` at
  `Dialog.TitleOffsetX/Y`. Controls are placed from the body's corner;
  buttons without a width span the body.
- Tiles are `vgui/menu_tiles/UI_tile_128_*` (corners, edges, interior); the
  tab's bottom row continues into the body.
- Footer buttons sit `FooterPanel.OffsetY` under the body from its left
  edge, `FooterPanel.ButtonGapX` apart, boxed labels in
  `FooterPanel.TextFont`.
- New Game / Load Game: a list 200x150 units at 0,25 (rows 25 units, label
  at 10,2, vertically centred), the chapter picture
  `vgui/chapters/chapter<N>` at 212,15 (225x152) and "Chapter N - Name"
  below it at 212,170; chapter names come from `SP_PRESENCE_TEXT_CH<N>`.
  Load Game lists saves newest first by date and marks auto and quick saves
  on the picture.
- In a game the pause menu (`ingamemainmenu.res`, no title) pauses the
  server; the scene behind it turns grey, blurred and darker (about 0.48 of
  the blurred luminance, fitted to a screenshot).
- UI sounds: `UI/menu_focus.wav`, `UI/menu_accept.wav`, `UI/menu_back.wav`,
  `UI/menu_invalid.wav`.

## Loading screen

- `LoadingProgress` picks a poster: the act's
  `vgui/loading_screens/loadingscreen_a<N>` when loading from the menu or
  entering an act's first map (`sp_a3_00`/`01` count as act 2, `sp_a3_03`
  always shows act 3), otherwise `loadingscreen_default_<letter>`; the
  credits map never shows the act poster. Every numbered frame
  `<poster>_<n>` (`_widescreen` when wide) that exists is shown in turn
  (`ui_loadingscreen_autotransition_time` 5 s, transitions 1 s).
- Our map loads block, so with the menus running a load waits two frames
  for the poster to be on screen first.

## Not done yet

Options dialogs, the save game dialog (Save Game makes a quick save), save
screenshots, the window flip transitions, controller footer glyphs, the
loading progress dots and spinner, instructor hints, and movie screens in
the world (`vgui_movie_display`).
