# Portal openings

How the installed game draws a portal's opening and rim. Gathered from the
installed `client.dll` (IDA), the shipped materials in
`materials/models/portals`, and the shipped compiled shaders
`shaders/fxc/portal_refract_vs20.vcs` and `portal_refract_ps20b.vcs`
(unpacked with `re/tools/vcs_dump.py` and disassembled with the Windows
SDK's `fxc /dumpbin`; their constant tables keep the parameter names), and
compared with screenshots from the game (`re/shots/m10_portals.txt`).
Written in our own words; no code is copied.

## Materials

The opening uses the `PortalRefract` shader in three stages:
`portal_refract_1` (stage 0, a refraction of the scene used while
switching), `portal_stencil_hole` (stage 1, marks the hole) and
`portalstaticoverlay_1` / `_2` (stage 2, the rim and static). The overlay
material sets `$PortalMaskTexture models/portals/noise-blur-256x256`,
`$PortalColorTexture models/portals/portal-blue-color` (orange for the
second portal; 256 x 1 sRGB gradients) and `$PortalColorScale 4.0`, with
proxies for the time, the open amount and the static amount. A tinted
variant takes two colours instead of the gradient (co-op colours).

## Open and static amounts (client)

- Each frame the open amount rises by `2 / host timescale × frame time` up
  to 1, so a new portal opens in half a second.
- The static amount is set to 1 when a portal activates (on it and on its
  partner) and falls by the frame time to 0. An unlinked portal always
  reports static 1.

## Hole (stage 1)

With `t` the texture coordinate across the portal quad (0 to 1) and
`d = |2t - 1|`, the hole is `d <= s²`, where `s` is the smoothstep
(`3x² - 2x³`) of the open amount.

## Rim and static (stage 2)

- Vertex side: the rim coordinate is `t × 1.075 - 0.0375`. Two noise
  coordinates start at `((t - 0.5) / clamp(open + 0.001, 0, 1) + 0.5) × 0.3`
  and scroll along x at +0.0275 and -0.0275 per second.
- Pixel side, with `c = 1 - static` (the "clarity"):
  - Three noise reads: one at the first coordinate, one at the second
    (its components swapped) pushed by `-0.02 ×` the first read, one at
    the first coordinate pushed by `-0.02 ×` the second read; `avg` is the
    mean of the last two reads' green channels.
  - The rim strength is, inside the hole, a ramp from 0 at `d = s² - 0.3`
    to 1 at `d = s²`, and outside a fade from 1 at `s²` to 0 at
    `s² + 0.075`; times `max(clamp(2.5 s), 1 - c)`.
  - `burn = 1 - smoothstep(clamp((avg - strength + 0.875) / 1.75))` and,
    inside the hole, `fill = 1 - smoothstep(clamp(c / avg))`.
  - `alpha = clamp(strength × burn + inside × fill) × 1.5`; the gradient is
    read at `alpha × √burn × (|t'_y|^1.5 × 0.8 + 0.2)` (brighter toward the
    bottom of the quad) and multiplied by the colour scale, then fogged.
  - The colour and alpha are multiplied by the light scale clamped to 1,
    so the fire does not brighten with the exposure in dark places.
- So an unlinked portal is filled with the fire (clarity 0 makes the fill
  1 everywhere inside), and once linked the fire shrinks back to the rim
  as the static fades.

# Player transit

How the installed game moves the player through a linked pair. Gathered
from the installed `server.dll` and `client.dll` (IDA): the portal-aware
game movement, `CProp_Portal`'s exit speed overrides, the player's enter
and exit sound think and `C_Portal_Player`'s view calculation. Facts only,
in our own words.

## Walking into the wall

- A portal cuts a rectangular hole (its half width and half height, 32 by
  56 for the player's portals) through whatever it sits on; the cut wall
  around the hole keeps the player in the tunnel. The surface is often not
  exactly at the portal: `sp_a1_intro1`'s map-placed orange portal sits 2
  units in front of its wall, so the cut is by the space behind the
  opening, not by the surface plane.

## When the player teleports

- After each move the game sweeps the player's box from where the move
  started to where it ended and looks at every open, linked portal the
  sweep touches. A portal is a candidate when the box's centre started in
  front of it (or the player is already in that portal's environment) and
  the centre ends behind its plane projected inside the rectangle, or
  still in front but with the box's deepest corner far enough behind that
  the line from the centre to that corner crosses the plane within the
  rectangle (1 unit of slack). The nearest candidate (portal origin to the
  box centre) wins.
- The player teleports through it only once the centre itself is behind
  the plane (a fixed portal); objects other than the player teleport when
  their origin (or physics centre) passes behind it and they overlap its
  hole.

## What changes

- Gravity: the fraction of the move left after the crossing is
  `1 - before / (before - after)` (signed distances of the centre). Unless
  on the ground, the velocity gained from gravity over that remaining time
  (`sv_gravity` × time, down) is taken off, the rest is turned through the
  portal, and the gravity part is added back unturned, scaled by 1.008.
- Exit speed: the exit portal gives a minimum speed along its facing and a
  maximum speed. For the player the minimum is 300 out of a floor portal
  (facing z > 0.866); out of one facing up between 0.5 and 0.866 it is the
  launch speed needed to fall clear of the portal's bottom edge, at most
  300; otherwise there is none. (Other objects: 225 out of a floor portal
  entered through a floor portal, else 50.) The maximum is 1000. Too slow
  along the facing is raised to the minimum; otherwise a speed over the
  maximum is scaled down to it. Each component is then clamped to
  `sv_maxvelocity`.
- Crouching: when the portal turns which way is up (the turned up vector's
  z below 0.866 in size), or the exit faces up between 0.1 and 0.9 and the
  player leaves it upward faster than 200 along its facing, a standing
  player is made to crouch at once.
- Position: the box centre is carried through the portal. When the player
  was crouched by a tilted fast exit, or by a turn with more than 200
  along the exit's facing, the box is shifted so its corners stay 5 units
  inside the exit's rectangle. If the new box is stuck, it is moved out
  along the line from the exit's axis (the box projected onto the line
  through the exit's centre along its facing) toward the wanted spot, and
  failing that to the nearest free space.
- View: the eye angles are turned by the full portal transform (roll
  included). The client keeps the camera continuous: its interpolation
  history gets the portal's transform inserted
  (`cl_portal_teleportation_interpolation_fixup_method 1`), and the old eye
  carried through the portal is remembered against the new eye.
- The `cl_portal_camera_orientation_*` variables exist but nothing reads
  them.

## Seeing through before the body goes

- When working out the view, if the eye has gone behind a portal's plane
  while the body is still in front, and the line from the body to the eye
  crosses inside the rectangle, the eye position and angles are carried
  through that portal; likewise right after a teleport if the eye is still
  behind the exit's partner. The view is never inside the wall.

- When the view's near plane reaches behind a portal inside its opening
  (the eye a little in front of it, or carried through and a little in
  front of the exit), the opening's quad is cut away by the near plane;
  the game fills that piece of the near plane with the portal's own
  material (its render fix mesh), so the wall behind never shows.

## Sounds

- `PortalPlayer.EnterPortal` plays when a box a quarter the size of the
  player's (about the same centre) touches the rectangle of the portal the
  player is in; `PortalPlayer.ExitPortal` once a box 0.3 the size no
  longer touches it, or the player leaves the portal's environment. After
  a teleport the player is in the exit portal's environment, so the exit
  sound comes as they step away from it. Volume is
  `min(0.3 + speed × 0.00075, 1)` and pitch `80 + speed × 0.03` percent.

## Map-placed portals

- Opening a `prop_portal` (`SetActivatedState 1`, `NewLocation`) runs the
  placement check, which settles the portal onto the surface behind it
  1/32 unit out and bumps it to fit. Measured in the game (`script
  printl(...GetOrigin())`): `sp_a1_intro1`'s orange portal, placed at
  x = -1137 in front of its wall at -1135, opens at -1135.03; the blue one,
  whose bottom edge was level with the floor, opens 1.31 units higher and
  0.14 to the side. Ours settles onto the surface but does not bump.
- Portals are 32 by 56 (half sizes): `CProp_Portal` takes 56 from a
  function returning it the first time one is built and 32 from data;
  `Portals_ResizeAll` changes both.

## Views

- Portal views nest two deep (`r_portal_stencil_depth 2`): a portal seen
  inside another portal's view shows its own view. Ours draws the second
  level with a second camera per portal and ends it there with the portal
  full of static.
- The player's body (`models/player/chell/player.mdl`) is drawn in the
  views through portals, never in the first-person view; walking through
  `sp_a1_intro1`'s pair you see yourself running in the glass room.

## Compared with the game

Walking `sp_a1_intro1`'s pair at host_timescale 0.2 (`getpos` every few
ticks, `re/out/ref_portal_walk`): the teleport comes as the centre passes
the plane, yaw goes from 0 to 90 exactly, speed carries through and the
exit lands at z 2674.85 (the bumped blue portal's height). The game steps
the player up 2.5 units onto the opening's lower edge where ours steps 1
(the cut hole's bottom sits 54.5 below the centre in the game; we cut at
the full 56).
