# Model lighting and shading

How the installed game lights and shades dynamic studio models (props,
NPCs, the first-person weapon): which world lights reach a model, how the
ambient cube is built, and what the phong variant of `VertexLitGeneric`
computes per pixel. Gathered from the installed `engine.dll` and
`shaderapidx9.dll` (IDA), the shipped compiled shaders
(`platform/shaders/fxc/*.vcs`, disassembled with the Windows SDK's
`fxc /dumpbin`; their constant tables still carry parameter names) and the
installed maps and materials. Written in our own words; no code is copied.

## World lights

- The world lights lump used by Portal 2 maps is version 1 (100-byte
  records): origin, intensity, normal, shadow cast offset, cluster, type,
  style, stopdot, stopdot2, exponent, radius, constant / linear / quadratic
  attenuation, flags, texinfo, owner. Types: 0 surface (texture light),
  1 point, 2 spot, 3 sky light, 4 "quake" light, 5 sky ambient.
- Flag bit 0 means the light's direct contribution is already part of the
  leaf ambient samples. In the shipped maps it is set on most surface lights
  (`sp_a1_intro1` has 16 without it) and on no point or spot light seen so
  far; point and spot lights carry flag value 2.
- Surface lights are stored with all three attenuation terms zero. The
  loader leaves them so, but gives point and spot lights with all-zero
  terms a quadratic term of 1, and a spot exponent of 0 becomes 1.
- A radius below 1 is replaced at load by the distance where the light's
  intensity (vector length) falls to 0.015 (HDR) or 0.03 (LDR) under its
  attenuation; with no linear or quadratic term the radius is 2000.

## From world light to renderer light

- Point: point light with the stored attenuation.
- Spot: spot light; the inner and outer cone cosines are stopdot and
  stopdot2 and the falloff exponent is the stored exponent.
- Surface: a spot light along the surface normal whose cone is the whole
  front hemisphere (both cone cosines 0, falloff exponent 1, and the
  reciprocal of the cone width left at 1, with the light marked as having
  its derived values computed) and pure inverse-square attenuation (terms
  0, 0, 1). In the vertex shader this works out to a cosine-weighted
  emitter: `cos / d²` in front, almost nothing behind.
- Sky light: directional light. Quake and sky ambient lights are never
  turned into renderer lights.
- Light styles scale the colour by `style value / 264` (264 is "normal").

The vertex shader turns each renderer light into one attenuation value per
vertex: `1 / (c + l·d + q·d²)`, times for spots
`min(pow(max((cos θ − cos outer) / (cos inner − cos outer), 0.0001), exponent), 1)`,
where θ is the angle between the spot axis and the direction from the light;
directional lights get 1.

## Which lights a model gets

The engine's light cache picks lights per cache point (the model's
lighting origin):

- Candidates are the world lights whose cluster is in the point's PVS (the
  sky light instead needs a leaf flagged as seeing the sky, and a ray from
  the point toward the sun whose first opaque world brush it enters through
  a sky face). Quake and sky ambient lights are skipped. A light hidden from
  the point by the world or a static prop (a line from the light to the
  point, opaque contents, no entities) gives nothing. Measured in
  `sp_a1_wakeup`: dormant GLaDOS under the chamber's open roof is lit by
  the sun (without it her white shell is a quarter as bright as the
  game's), and the training door at the end of the hall gets only its
  ambient cube, as a chamber spot light 777 units away shines at it through
  the hall's windows, which are static props. Surface lights reach only as far as their
  radius; the other kinds may reach 1000 times theirs
  (`r_lightcache_radiusfactor`), in practice without limit.
- With `r_radiosity 4` (the default) and the ambient cube taken from the
  leaf samples, lights with flag bit 0 are skipped entirely: they are
  already in the ambient cube. This is why surface lights normally never act
  as local lights on models. (Static props lit at run time take a different
  ambient path, not covered here.)
- Each candidate is scored by the luma of its intensity (weights 0.299,
  0.587, 0.114) times its falloff at the point (distance attenuation, cone,
  and a visibility estimate). Lights other than surface lights need a score
  of at least `r_worldlightmin` (0.0002).
- The best `r_worldlights` (3) lights, further capped by the hardware's
  light count, become local lights. Every candidate pushed out of that list
  is added to the ambient cube instead: each of the six faces gains
  `intensity × falloff × max(0, face direction · direction to the light)`.
- `mat_ambient_light_r/g/b` (all 0) are added to every face at the end.

## Leaf ambient cube

- The ambient cube at a point blends every sample of the containing leaf,
  each weighted by `1 / (distance² + 1)`. A sample's position is the leaf's
  minimum corner plus `byte / 255` of the leaf's size per axis.
- A leaf with no samples may name another leaf in its `first_sample` field;
  that leaf's samples (and bounds) are used instead.
- Sample colours are RGBE like lightmap texels but are decoded without the
  `1 / 255` lightmap normalisation (255 times brighter for the same bytes).

## Phong shading (`VertexLitGeneric` with `$phong 1`)

Phong materials use the `phong_ps30` pixel shader. Per pixel, with N the
(bumped) world normal, E the unit vector to the eye and, per local light,
L the unit vector to the light and A its colour times its attenuation:

- Diffuse: `ambient cube(N) + Σ A × shade`. Without half-Lambert `shade` is
  `saturate(N·L)`; with it, `saturate(N·L × 0.5 + 0.5)²`. With
  `$lightwarptexture` (phong materials only) the light term becomes
  `A × 2 × warp(u, u)` instead, `u` being `saturate(N·L)` or the unsquared
  half-Lambert value. The ambient cube gets neither treatment.
- The diffuse result is multiplied by the base texture and by the material
  colour (`$color2`), which tints the diffuse term only (a shader constant
  can fade that tint by base alpha).
- Self-illumination replaces the diffuse result by `$selfillumtint × base`
  in proportion to the mask: the `$selfillummask` texture (RGB) when given,
  otherwise base alpha. Specular and reflections are added after this.
- Specular: `Σ A × saturate(N·L) × saturate(R·L)^exponent` with
  `R = reflect(−E, N)`. The ambient cube contributes no specular. The sum is
  multiplied by:
  - the mask: normal map alpha by default, base alpha with
    `$basemapalphaphongmask`, base luminance (0.3, 0.59, 0.11) with
    `$basemapluminancephongmask`;
  - `$phongboost`;
  - the tint: `$phongtint`, or with `$phongalbedotint` a blend from white to
    the base colour by the exponent texture's green channel, else white;
  - the Fresnel term below.
- Exponent: `$phongexponent` when set; otherwise taken from the exponent
  texture's red channel as `1 + 149 × red` (so 1..150).
- Fresnel: with `f = (1 − saturate(N·E))²` and `$phongfresnelranges`
  `[a b c]`, the factor is `lerp(a, b, 2f)` for `f ≤ 0.5` and
  `lerp(b, c, 2f − 1)` above. The ranges are used as written, so
  `[5 1 2]` (the portal gun) makes facing highlights five times stronger.
- Cube map: `cube(reflect(−E, N)) × $envmaptint`, scaled by
  `lerp(1, Fresnel, $envmapfresnel)` and masked by base alpha, or with
  `$normalmapalphaenvmapmask` by the specular mask above (the material flag
  goes straight into a shader constant, normal map or not). With
  `$invertphongmask` that reflection mask becomes one minus itself; the
  highlights keep theirs, whatever the parameter's help text says ("invert
  the phong mask"). Phong has no `$envmapmask` texture and no
  `$envmapcontrast` / `$envmapsaturation`, and its tint is passed as written
  (not converted from gamma). `$basemapalphaenvmapmask`, which some glass
  materials set, is not a parameter of the shader and does nothing.
- Everything is then multiplied by screen-space ambient occlusion and the
  tone mapping scale, and fogged.
- The exponent texture defaults to white when a material has none, so
  `$phongalbedotint` alone tints highlights fully by the base colour.
  (Assumed; the sampler's fallback binding has not been traced.)

The half-Lambert variant is chosen when `$phongdisablehalflambert` is
undefined or zero (`stdshader_dx9.dll`), so phong materials use half-Lambert
by default. Non-phong `VertexLitGeneric` uses it only with `$halflambert`.

## Reflections without phong

Plain `VertexLitGeneric` (`vertexlit_and_unlit_generic_ps30`, and the
`_bump_` variant for materials with a normal map) builds its reflection as
`cube(R) × mask × $envmaptint`, then applies contrast,
`lerp(c, c², $envmapcontrast)`, and saturation,
`lerp(luminance, c, $envmapsaturation)` with weights 0.299, 0.587, 0.114,
and adds the result after the diffuse term. The mask is the red channel of
`$envmapmask`, base alpha with `$basealphaenvmapmask` or normal map alpha
with `$normalmapalphaenvmapmask` (1 without any). The tint is passed as
written, not converted from gamma; the bumped variant uses only the first
component of the contrast.

## Which cube map a model gets

- At map load the engine reads the cube map samples (origin and size) and
  loads `materials/maps/<map>/c<x>_<y>_<z>` for each, the `.hdr` version in
  HDR mode, falling back to the plain one, then the map's
  `cubemapdefault`, then `engine/defaultcubemap`. The first sample's texture
  becomes the default cube map (`cubemapdefault` when the map has none).
- A model whose materials use `env_cubemap` (a studio header flag set when
  its materials load) is drawn with the sample nearest its lighting origin,
  by straight distance with no visibility test; on ties the earlier sample
  wins. The choice is part of the light cache entry, so it is made once.
- Static props: the lighting origin is the lump's lighting origin when the
  prop has the "use lighting origin" flag (2), otherwise the model's
  illumination point (`illumposition`, or an attachment if the model names
  one) placed with the prop's origin and angles; the prop's scale plays no
  part. This holds whether or not the prop has baked vertex lighting.
- Dynamic models: their light cache entry at the lighting origin (the one
  the entity provides, else the illumination point placed with the model).
- The baked HDR cube maps are RGBA16F with small texels (the brightest in
  `sp_a1_intro7` is about 0.04). Used as stored, reflections match the
  game, for example the puddle in `sp_a1_intro7`, which reflects in the same
  place and shape; ours is somewhat stronger relative to the floor beneath,
  whose own brightness is lower in ours.
