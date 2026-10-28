# Refract shader

How the installed game draws materials using the `Refract` shader: glass
that shows the scene behind it, or with `$localrefract` a picture of a light
fixture seen through glass. Gathered from the installed `stdshader_dx9.dll`
and `shaderapidx9.dll` (IDA), the shipped compiled shaders
`platform/shaders/fxc/refract_vs20.vcs` and `refract_ps20b.vcs`
(disassembled with the Windows SDK's `fxc /dumpbin`; their constant tables
keep the parameter names), the shipped materials, and screenshots from the
game. Written in our own words; no code is copied.

## Where it is used

- Only on studio models. No brush face in any shipped single-player map
  uses a `Refract` material; the remaining `Refract` materials are
  particles and a HUD element.
- By far the most common user is `models/props_lab/glass_lightcover.mdl`, a
  32 x 128 unit quad with three skins (`glass/glasswindow_refract01`, `_warm`,
  `_neutral`). Single-player maps place it as a static prop by the dozen
  (`sp_a2_bts1` has over 200, `sp_a1_wakeup` 12). These materials use local
  refraction.
- Observation windows (`models/props_lab/glass_observation_*.mdl`,
  `glasswindow_observation`), the breakable panes in `props_destruction`, and
  the neurotoxin tube use the scene behind.

## Shader files and combos

There is no `refract_ps30`; the game pairs `refract_vs20` with
`refract_ps20b` (or `refract_ps20` on older hardware).

Vertex shader static combos: model (bit 0, the material's `$model` flag) and
colour modulate (bit 1, `$vertexcolormodulate`). Dynamic: compressed vertices
and skinning.

Pixel shader static combos, lowest bit first: magnify (`$magnifyenable`),
blur (`$bluramount` as a whole number, clamped to 0..1, so `.3` means no
blur), fade out on silhouette, cube map (`$envmap` is a texture), refract
tint texture, masked, colour modulate, second normal map (`$normalmap2`), an
always-zero slot (shader-side sRGB reads, present only in the ps20b build),
and local refraction (`$localrefract`). Dynamic: height fog, depth written to
destination alpha, stereo. With local refraction the blur and magnify bits
make no difference (those combos are stored as aliases of the ones without).

## Parameters and defaults

Defaults the shader fills in when a material leaves a parameter out:
`$envmaptint` [1 1 1], `$envmapcontrast` 0, `$envmapsaturation` 1,
`$bluramount` 0, `$localrefract` 0, `$localrefractdepth` 0.05, and 0 for the
masked, fade, magnify and viewport flags. `$refracttint` ([1 1 1]) and
`$refractamount` (2) only have defaults in the parameter table; every shipped
model material sets the amount.

A material without local refraction is always flagged translucent, so it is
drawn after opaque geometry.

Samplers: 2 is the refraction source, the base texture when the material has
one and otherwise the copy of the frame buffer; 3 the normal map; 1 the
second normal map; 4 the cube map; 5 the refract tint texture. The refraction
source, cube map and tint texture are read as sRGB, the normal maps as
linear, and the output is written as sRGB.

Pixel constants:

- c0 `$envmaptint` and c1 `$refracttint`, both converted from gamma to
  linear (below); c2 `$envmapcontrast`; c3 `$envmapsaturation` (both per
  channel).
- c5.x `$refractamount`.
- c6 the magnify centre and the reciprocal of `$magnifyscale`.
- c7 = (height / width of the refraction source, 1, `$localrefractdepth`).
  The ratio is a whole-number division of the texture's sizes, so a 128 x 512
  texture gives 4 and a wider-than-tall one gives 0.
- c11 eye position, c12 fog parameters, c29 fog colour, c30 the engine's
  light scales (below).

### Gamma to linear

Colour parameters such as the two tints go through the game's gamma table:
a channel above 1 is passed unchanged, one from 0.95 to 1 becomes exactly 1,
anything else is rounded to the nearest of 256 steps and raised to the power
2.2. So `{250 250 250}` is pure white and `[.81 .89 1.0]` becomes about
(0.62, 0.77, 1.0).

## Vertex shader

Besides the clip position the vertex shader hands the pixel shader:

- the normal map coordinates (the texture coordinate through
  `$bumptransform`, and through `$bumptransform2` for the second map);
- the eye vector, normalised per vertex and expressed in **tangent space**;
- the world normal, tangent and binormal, each normalised; the binormal is
  normal × tangent times the tangent's sign;
- the screen position in projective form (x and y mapped to 0..1 with y
  down, plus w), giving the screen coordinate after division;
- the world position.

## Pixel shader

`n` is the normal map sample mapped to -1..1 and left unnormalised, `a` the
normal map's alpha, `T`, `B`, `N` the interpolated frame, and `E` the
tangent-space eye vector.

Refracted image, scene mode:

- The screen coordinate is shifted by `n.xy × a × $refractamount`. The
  tangent-space x and y are added to the screen coordinate directly, with no
  conversion to screen axes, and the normal map's alpha scales the shift.
- Without blur this is a single lookup. With blur it is four lookups offset
  by (-1, -1), (2, -1), (-1, 2) and (2, 2) 1024ths of the screen, weighted 4,
  2, 2 and 1 ninths: a small tent filter whose size does not depend on the
  resolution.

Refracted image, local mode (`$localrefract` with a base texture):

- `V` is the direction from the pixel to the eye, recomputed per pixel and
  taken into tangent space. The lookup into the base texture is shifted by
  `(n.xy − V.xy × (2 − n.z) / V.z) × depth`, with the shift in u multiplied
  by the height / width ratio from c7, and the coordinate clamped to 0..1.
  The effect is a picture lying `depth` texture heights behind the glass,
  seen with parallax.
- A second lookup at the coordinate shifted by `n.xy × 0.1` supplies only
  its alpha: the result is 97.5% of the first lookup's colour and 2.5% of
  that alpha (grey).
- The result is multiplied by `n.z³`, which darkens the ridges of the normal
  map. The scene mode has no such term.

Tint: the refracted image is multiplied by c1, and with a tint texture by
`2 × tint texture × c1` (the texture sampled at the normal map coordinate).

Reflection (cube map combos only):

- The lookup direction is the reflection of the **tangent-space** eye vector
  about the **world-space** bumped normal (`n.x T + n.y B + n.z N`), computed
  without normalising. The spaces really are mixed; reproducing it keeps
  reflections where the game puts them.
- The sample is multiplied by c30.z, by the normal map's alpha and by the
  envmap tint, then contrast (`lerp(c, c², contrast)`) and saturation
  (`lerp(luminance, c, saturation)`, luminance weights 0.299, 0.587, 0.114)
  are applied in that order, after the tint.
- Finally it is scaled by `0.6 + 0.4 × (1 − saturate(n · E))`, from 0.6
  facing the viewer to 1 edge-on. `$fresnelreflection` has a parameter slot
  but is not passed to the shader.
- With `$envmap env_cubemap` the cube map is the baked sample nearest the
  prop's lighting origin, as for other models (see `model_shading.md`).
  On upright windows the mixed-space reflection vector mostly points down,
  so from ordinary viewpoints the reflection picks up the floor and stays
  faint, in the game as here.

The output is `refracted × tint + reflection`, fogged with the same range
fog as the game's phong shader (by distance from the eye, the factor
squared), with the normal map's alpha as output alpha. We fog it like our
other model and world shaders instead.

## Blending

With a cube map the shader sets no blending at all: the result replaces what
is behind (which is already part of it). Without one it blends like the
game's other shaders, using the normal map as the "base texture": alpha
blending when the normal map has alpha, `$alpha` is below 1 or vertex alpha
is on; additive with `$additive`. Depth is written unless `$nowritez` (or
blending is on).

## Light scales and HDR

- c30 is set by the shader API whenever the tone map scale changes: x is the
  tone map scale, z the cube map scale (16 in integer HDR, 1 in float HDR and
  LDR), y and w derived from x.
- Unlike the game's lit shaders, the refract shader never multiplies by the
  tone map scale. In scene mode the frame buffer copy already includes it;
  in local mode the base texture is shown at its own brightness. Captures
  with `mat_force_tonemap_scale 0.5` and `2` confirm it: the light covers in
  `sp_a1_wakeup` keep exactly the same pixels while the walls change by a
  factor of two.
- The HDR cube map these materials use (`metal/black_wall_envmap_002a_hdr`)
  is RGBA16F holding values up to 1. Taken times 16 the covers would wash
  out, which the game does not show; used at the stored value they match
  (so do the maps' own baked cube maps on other models, see
  `model_shading.md`). How the factor 16 is cancelled for float cube maps
  has not been traced.

## Compared with the game

At the start of `sp_a1_wakeup` (`re/shots/models.txt`) the covers match the
game's capture with bloom disabled (`mat_bloomscale 0`) to within about two
levels of 255 on average (mean absolute error about 5 over each cover). The
default capture is about 10% brighter on the covers and smoother across the
tubes: that is the game's bloom, not the shader.

## Not implemented here

Masking by destination alpha, fading at silhouettes, magnification, the
second normal map, vertex colour modulation, mirroring at the viewport
edges, `$bumptransform` and material proxies (the neurotoxin tube's
animated normal map stays on its first frame). All transmissive surfaces see
one copy of the opaque pass, so overlapping panes do not see each other,
while the game copies the frame buffer before each refracting model.
