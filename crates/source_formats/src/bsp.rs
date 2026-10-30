//! Source BSP (version 19-21) reader, with the Portal 2 lump layouts.

use glam::Vec3;

use crate::reader::{Reader, read_records, str_from_nul};
use crate::{Error, Result};

pub mod lump {
    pub const ENTITIES: usize = 0;
    pub const PLANES: usize = 1;
    pub const TEXDATA: usize = 2;
    pub const VERTEXES: usize = 3;
    pub const VISIBILITY: usize = 4;
    pub const NODES: usize = 5;
    pub const TEXINFO: usize = 6;
    pub const FACES: usize = 7;
    pub const LIGHTING: usize = 8;
    pub const LEAFS: usize = 10;
    pub const EDGES: usize = 12;
    pub const SURFEDGES: usize = 13;
    pub const MODELS: usize = 14;
    pub const WORLDLIGHTS: usize = 15;
    pub const LEAFFACES: usize = 16;
    pub const LEAFBRUSHES: usize = 17;
    pub const BRUSHES: usize = 18;
    pub const BRUSHSIDES: usize = 19;
    pub const AREAS: usize = 20;
    pub const AREAPORTALS: usize = 21;
    pub const DISPINFO: usize = 26;
    pub const ORIGINALFACES: usize = 27;
    pub const PHYSDISP: usize = 28;
    pub const PHYSCOLLIDE: usize = 29;
    pub const VERTNORMALS: usize = 30;
    pub const VERTNORMALINDICES: usize = 31;
    pub const DISP_LIGHTMAP_ALPHAS: usize = 32;
    pub const DISP_VERTS: usize = 33;
    pub const DISP_LIGHTMAP_SAMPLE_POSITIONS: usize = 34;
    pub const GAME_LUMP: usize = 35;
    pub const LEAFWATERDATA: usize = 36;
    pub const PRIMITIVES: usize = 37;
    pub const PRIMVERTS: usize = 38;
    pub const PRIMINDICES: usize = 39;
    pub const PAKFILE: usize = 40;
    pub const CLIPPORTALVERTS: usize = 41;
    pub const CUBEMAPS: usize = 42;
    pub const TEXDATA_STRING_DATA: usize = 43;
    pub const TEXDATA_STRING_TABLE: usize = 44;
    pub const OVERLAYS: usize = 45;
    pub const LEAFMINDISTTOWATER: usize = 46;
    pub const FACE_MACRO_TEXTURE_INFO: usize = 47;
    pub const DISP_TRIS: usize = 48;
    pub const WATEROVERLAYS: usize = 50;
    pub const LEAF_AMBIENT_INDEX_HDR: usize = 51;
    pub const LEAF_AMBIENT_INDEX: usize = 52;
    pub const LIGHTING_HDR: usize = 53;
    pub const WORLDLIGHTS_HDR: usize = 54;
    pub const LEAF_AMBIENT_LIGHTING_HDR: usize = 55;
    pub const LEAF_AMBIENT_LIGHTING: usize = 56;
    pub const FACES_HDR: usize = 58;
    pub const MAP_FLAGS: usize = 59;
    pub const OVERLAY_FADES: usize = 60;
}

pub mod contents {
    pub const EMPTY: u32 = 0;
    pub const SOLID: u32 = 0x1;
    pub const WINDOW: u32 = 0x2;
    pub const AUX: u32 = 0x4;
    pub const GRATE: u32 = 0x8;
    pub const SLIME: u32 = 0x10;
    pub const WATER: u32 = 0x20;
    pub const BLOCKLOS: u32 = 0x40;
    pub const OPAQUE: u32 = 0x80;
    pub const TESTFOGVOLUME: u32 = 0x100;
    pub const TEAM1: u32 = 0x800;
    pub const TEAM2: u32 = 0x1000;
    pub const IGNORE_NODRAW_OPAQUE: u32 = 0x2000;
    pub const MOVEABLE: u32 = 0x4000;
    pub const AREAPORTAL: u32 = 0x8000;
    pub const PLAYERCLIP: u32 = 0x10000;
    pub const MONSTERCLIP: u32 = 0x20000;
    pub const ORIGIN: u32 = 0x100_0000;
    pub const MONSTER: u32 = 0x200_0000;
    pub const DEBRIS: u32 = 0x400_0000;
    pub const DETAIL: u32 = 0x800_0000;
    pub const TRANSLUCENT: u32 = 0x1000_0000;
    pub const LADDER: u32 = 0x2000_0000;
    pub const HITBOX: u32 = 0x4000_0000;
}

pub mod surf {
    pub const LIGHT: u32 = 0x1;
    pub const SKY2D: u32 = 0x2;
    pub const SKY: u32 = 0x4;
    pub const WARP: u32 = 0x8;
    pub const TRANS: u32 = 0x10;
    pub const NOPORTAL: u32 = 0x20;
    pub const TRIGGER: u32 = 0x40;
    pub const NODRAW: u32 = 0x80;
    pub const HINT: u32 = 0x100;
    pub const SKIP: u32 = 0x200;
    pub const NOLIGHT: u32 = 0x400;
    pub const BUMPLIGHT: u32 = 0x800;
    pub const NOSHADOWS: u32 = 0x1000;
    pub const NODECALS: u32 = 0x2000;
    pub const NOPAINT: u32 = 0x4000;
    pub const HITBOX: u32 = 0x8000;
}

#[derive(Clone, Copy, Debug)]
pub struct LumpInfo {
    pub offset: usize,
    pub length: usize,
    pub version: i32,
    pub uncompressed_size: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub normal: Vec3,
    pub dist: f32,
    pub kind: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct TexData {
    pub reflectivity: Vec3,
    pub name_index: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct TexInfo {
    pub texture_vecs: [[f32; 4]; 2],
    pub lightmap_vecs: [[f32; 4]; 2],
    pub flags: u32,
    pub texdata: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct Face {
    pub plane: u16,
    pub side: u8,
    pub on_node: u8,
    pub first_edge: i32,
    pub num_edges: i16,
    pub texinfo: i16,
    pub dispinfo: i16,
    pub fog_volume: i16,
    pub styles: [u8; 4],
    pub light_offset: i32,
    pub area: f32,
    pub lightmap_mins: [i32; 2],
    pub lightmap_size: [i32; 2],
    pub orig_face: i32,
    pub num_prims: u16,
    pub first_prim: u16,
    pub smoothing_groups: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub plane: i32,
    pub children: [i32; 2],
    pub mins: [i16; 3],
    pub maxs: [i16; 3],
    pub first_face: u16,
    pub num_faces: u16,
    pub area: i16,
}

#[derive(Clone, Copy, Debug)]
pub struct Leaf {
    pub contents: u32,
    pub cluster: i16,
    pub area: u16,
    pub flags: u16,
    pub mins: [i16; 3],
    pub maxs: [i16; 3],
    pub first_leaf_face: u16,
    pub num_leaf_faces: u16,
    pub first_leaf_brush: u16,
    pub num_leaf_brushes: u16,
    pub water_data: i16,
}

#[derive(Clone, Copy, Debug)]
pub struct Model {
    pub mins: Vec3,
    pub maxs: Vec3,
    pub origin: Vec3,
    pub head_node: i32,
    pub first_face: i32,
    pub num_faces: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct Brush {
    pub first_side: i32,
    pub num_sides: i32,
    pub contents: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct BrushSide {
    pub plane: u16,
    pub texinfo: i16,
    pub dispinfo: i16,
    pub bevel: u8,
    pub thin: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct DispInfo {
    pub start_position: Vec3,
    pub disp_vert_start: i32,
    pub disp_tri_start: i32,
    pub power: i32,
    pub min_tess: i32,
    pub smoothing_angle: f32,
    pub contents: u32,
    pub map_face: u16,
    pub lightmap_alpha_start: i32,
    pub lightmap_sample_position_start: i32,
}

impl DispInfo {
    pub fn side_verts(&self) -> usize {
        (1usize << self.power) + 1
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DispVert {
    pub vec: Vec3,
    pub dist: f32,
    pub alpha: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Cubemap {
    pub origin: [i32; 3],
    pub size: i32,
}

#[derive(Clone, Debug)]
pub struct Overlay {
    pub id: i32,
    pub texinfo: i16,
    pub face_count: u16,
    pub render_order: u16,
    pub faces: Vec<i32>,
    pub u: [f32; 2],
    pub v: [f32; 2],
    pub uv_points: [Vec3; 4],
    pub origin: Vec3,
    pub basis_normal: Vec3,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LeafAmbientIndex {
    pub sample_count: u16,
    pub first_sample: u16,
}

/// One ambient-cube sample; `cube` holds +X, -X, +Y, -Y, +Z, -Z in linear RGB
/// on the same scale as decoded lightmaps.
#[derive(Clone, Copy, Debug)]
pub struct LeafAmbientSample {
    pub cube: [Vec3; 6],
    /// Position inside the leaf bounds, 0-255 per axis.
    pub pos: [u8; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct WorldLight {
    pub origin: Vec3,
    pub intensity: Vec3,
    pub normal: Vec3,
    pub cluster: i32,
    pub kind: i32,
    pub style: i32,
    pub stopdot: f32,
    pub stopdot2: f32,
    pub exponent: f32,
    pub radius: f32,
    pub constant_attn: f32,
    pub linear_attn: f32,
    pub quadratic_attn: f32,
    pub flags: i32,
    pub texinfo: i32,
    pub owner: i32,
}

#[derive(Clone, Debug)]
pub struct GameLump {
    pub id: [u8; 4],
    pub flags: u16,
    pub version: u16,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct StaticProp {
    pub origin: Vec3,
    pub angles: Vec3,
    pub model: u16,
    pub first_leaf: u16,
    pub leaf_count: u16,
    pub solid: u8,
    pub flags: u8,
    pub skin: i32,
    pub fade_min: f32,
    pub fade_max: f32,
    pub lighting_origin: Vec3,
    pub forced_fade_scale: f32,
    pub diffuse_modulation: [u8; 4],
    pub uniform_scale: f32,
}

#[derive(Clone, Debug, Default)]
pub struct StaticProps {
    pub version: u16,
    pub models: Vec<String>,
    pub leaves: Vec<u16>,
    pub props: Vec<StaticProp>,
}

/// Map entity: ordered key/value pairs (duplicate keys are legal, e.g. outputs).
#[derive(Clone, Debug, Default)]
pub struct Entity {
    pub pairs: Vec<(String, String)>,
}

impl Entity {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.pairs.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }

    pub fn classname(&self) -> &str {
        self.get("classname").unwrap_or("")
    }

    pub fn targetname(&self) -> Option<&str> {
        self.get("targetname")
    }

    pub fn get_vec3(&self, key: &str) -> Option<Vec3> {
        self.get(key).and_then(crate::kv::parse_vec3)
    }

    pub fn origin(&self) -> Vec3 {
        self.get_vec3("origin").unwrap_or(Vec3::ZERO)
    }

    /// Pitch / yaw / roll in degrees (Source `angles` key).
    pub fn angles(&self) -> Vec3 {
        let mut a = self.get_vec3("angles").unwrap_or(Vec3::ZERO);
        // Some entities use "pitch" separately (lights).
        if let Some(p) = self.get("pitch") {
            a.x = crate::kv::parse_f32_lenient(p);
        }
        a
    }

    pub fn get_f32(&self, key: &str) -> Option<f32> {
        self.get(key).map(crate::kv::parse_f32_lenient)
    }

    pub fn get_i32(&self, key: &str) -> Option<i32> {
        self.get(key).map(crate::kv::parse_i32_lenient)
    }
}

/// Parses the entity lump text.
pub fn parse_entities(text: &str) -> Vec<Entity> {
    let b = text.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    let mut cur: Option<Entity> = None;
    let mut pending_key: Option<String> = None;
    while i < b.len() {
        match b[i] {
            b'{' => {
                cur = Some(Entity::default());
                pending_key = None;
                i += 1;
            }
            b'}' => {
                if let Some(e) = cur.take() {
                    out.push(e);
                }
                i += 1;
            }
            b'"' => {
                let start = i + 1;
                let mut j = start;
                while j < b.len() && b[j] != b'"' {
                    j += 1;
                }
                let s = String::from_utf8_lossy(&b[start..j]).into_owned();
                i = j + 1;
                if let Some(e) = cur.as_mut() {
                    match pending_key.take() {
                        None => pending_key = Some(s),
                        Some(k) => e.pairs.push((k, s)),
                    }
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// Decompressed potentially-visible-set data.
#[derive(Clone, Debug, Default)]
pub struct Visibility {
    pub num_clusters: usize,
    offsets: Vec<[i32; 2]>,
    data: Vec<u8>,
}

impl Visibility {
    fn parse(data: &[u8]) -> Result<Option<Self>> {
        if data.len() < 4 {
            return Ok(None);
        }
        let mut r = Reader::new(data);
        let n = r.i32()?.max(0) as usize;
        let mut offsets = Vec::with_capacity(n);
        for _ in 0..n {
            offsets.push([r.i32()?, r.i32()?]);
        }
        Ok(Some(Self { num_clusters: n, offsets, data: data.to_vec() }))
    }

    /// Returns the PVS bit vector for a cluster (1 bit per cluster).
    pub fn pvs(&self, cluster: usize) -> Vec<u8> {
        self.decompress(cluster, 0)
    }

    pub fn pas(&self, cluster: usize) -> Vec<u8> {
        self.decompress(cluster, 1)
    }

    fn decompress(&self, cluster: usize, kind: usize) -> Vec<u8> {
        let row = self.num_clusters.div_ceil(8);
        let mut out = vec![0u8; row];
        let Some(off) = self.offsets.get(cluster) else {
            out.fill(0xFF);
            return out;
        };
        let mut i = off[kind] as usize;
        let mut o = 0;
        while o < row && i < self.data.len() {
            if self.data[i] != 0 {
                out[o] = self.data[i];
                o += 1;
                i += 1;
            } else {
                let count = *self.data.get(i + 1).unwrap_or(&0) as usize;
                o += count;
                i += 2;
            }
        }
        out
    }
}

pub struct Bsp {
    pub version: i32,
    pub map_revision: i32,
    pub lumps: Vec<LumpInfo>,
    pub entities: Vec<Entity>,
    pub planes: Vec<Plane>,
    pub texdata: Vec<TexData>,
    pub texdata_strings: Vec<String>,
    pub vertices: Vec<Vec3>,
    pub nodes: Vec<Node>,
    pub texinfo: Vec<TexInfo>,
    pub faces: Vec<Face>,
    /// Lightmap samples (ColorRGBExp32), HDR when available.
    pub lighting: Vec<u8>,
    pub lighting_is_hdr: bool,
    pub leafs: Vec<Leaf>,
    pub edges: Vec<[u16; 2]>,
    pub surfedges: Vec<i32>,
    pub models: Vec<Model>,
    pub leaf_faces: Vec<u16>,
    pub leaf_brushes: Vec<u16>,
    pub brushes: Vec<Brush>,
    pub brush_sides: Vec<BrushSide>,
    pub disp_infos: Vec<DispInfo>,
    pub disp_verts: Vec<DispVert>,
    pub disp_tris: Vec<u16>,
    pub visibility: Option<Visibility>,
    pub game_lumps: Vec<GameLump>,
    pub static_props: StaticProps,
    pub pakfile: Vec<u8>,
    pub cubemaps: Vec<Cubemap>,
    pub overlays: Vec<Overlay>,
    pub leaf_ambient_index: Vec<LeafAmbientIndex>,
    pub leaf_ambient_samples: Vec<LeafAmbientSample>,
    pub world_lights: Vec<WorldLight>,
    pub phys_collide: Vec<u8>,
    pub vert_normals: Vec<Vec3>,
    pub vert_normal_indices: Vec<u16>,
    pub primitives: Vec<Primitive>,
    pub prim_verts: Vec<Vec3>,
    pub prim_indices: Vec<u16>,
}

#[derive(Clone, Copy, Debug)]
pub struct Primitive {
    pub kind: u8,
    pub first_index: u16,
    pub index_count: u16,
    pub first_vert: u16,
    pub vert_count: u16,
}

/// Converts a ColorRGBExp32 sample to linear RGB.
pub fn rgbexp_to_linear(c: [u8; 4]) -> Vec3 {
    let e = (c[3] as i8) as i32;
    let scale = 2f32.powi(e) / 255.0;
    Vec3::new(c[0] as f32 * scale, c[1] as f32 * scale, c[2] as f32 * scale)
}

impl Bsp {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let magic = r.bytes(4)?;
        if magic != b"VBSP" {
            return Err(Error::Magic { expected: "VBSP", found: magic.to_vec() });
        }
        let version = r.i32()?;
        if !(19..=21).contains(&version) {
            return Err(Error::Version(version as i64));
        }
        let mut lumps = Vec::with_capacity(64);
        for _ in 0..64 {
            lumps.push(LumpInfo { offset: r.i32()? as usize, length: r.i32()? as usize, version: r.i32()?, uncompressed_size: r.i32()? });
        }
        let map_revision = r.i32()?;

        let lump_data = |idx: usize| -> Result<std::borrow::Cow<'_, [u8]>> {
            let l = lumps[idx];
            if l.length == 0 {
                return Ok(std::borrow::Cow::Borrowed(&[]));
            }
            let raw = data.get(l.offset..l.offset + l.length).ok_or(Error::Eof)?;
            if l.uncompressed_size != 0 && raw.starts_with(b"LZMA") {
                Ok(std::borrow::Cow::Owned(crate::zip::decode_valve_lzma_header(raw)?))
            } else {
                Ok(std::borrow::Cow::Borrowed(raw))
            }
        };

        let entities = parse_entities(&String::from_utf8_lossy(&lump_data(lump::ENTITIES)?));
        let planes = read_records(&lump_data(lump::PLANES)?, 20, |r| Ok(Plane { normal: r.vec3()?, dist: r.f32()?, kind: r.i32()? }))?;
        let texdata = read_records(&lump_data(lump::TEXDATA)?, 32, |r| {
            Ok(TexData { reflectivity: r.vec3()?, name_index: r.i32()?, width: r.i32()?, height: r.i32()? })
        })?;
        let string_data = lump_data(lump::TEXDATA_STRING_DATA)?;
        let texdata_strings = read_records(&lump_data(lump::TEXDATA_STRING_TABLE)?, 4, |r| {
            let off = r.i32()? as usize;
            Ok(crate::reader::cstr_at(&string_data, off))
        })?;
        let vertices = read_records(&lump_data(lump::VERTEXES)?, 12, |r| r.vec3())?;
        let nodes = read_records(&lump_data(lump::NODES)?, 32, |r| {
            Ok(Node {
                plane: r.i32()?,
                children: [r.i32()?, r.i32()?],
                mins: [r.i16()?, r.i16()?, r.i16()?],
                maxs: [r.i16()?, r.i16()?, r.i16()?],
                first_face: r.u16()?,
                num_faces: r.u16()?,
                area: r.i16()?,
            })
        })?;
        let texinfo = read_records(&lump_data(lump::TEXINFO)?, 72, |r| {
            let mut tv = [[0f32; 4]; 2];
            let mut lv = [[0f32; 4]; 2];
            for row in tv.iter_mut() {
                for v in row.iter_mut() {
                    *v = r.f32()?;
                }
            }
            for row in lv.iter_mut() {
                for v in row.iter_mut() {
                    *v = r.f32()?;
                }
            }
            Ok(TexInfo { texture_vecs: tv, lightmap_vecs: lv, flags: r.i32()? as u32, texdata: r.i32()? })
        })?;

        let parse_faces = |bytes: &[u8]| {
            read_records(bytes, 56, |r| {
                Ok(Face {
                    plane: r.u16()?,
                    side: r.u8()?,
                    on_node: r.u8()?,
                    first_edge: r.i32()?,
                    num_edges: r.i16()?,
                    texinfo: r.i16()?,
                    dispinfo: r.i16()?,
                    fog_volume: r.i16()?,
                    styles: r.array()?,
                    light_offset: r.i32()?,
                    area: r.f32()?,
                    lightmap_mins: [r.i32()?, r.i32()?],
                    lightmap_size: [r.i32()?, r.i32()?],
                    orig_face: r.i32()?,
                    num_prims: r.u16()?,
                    first_prim: r.u16()?,
                    smoothing_groups: r.u32()?,
                })
            })
        };
        let hdr_lighting = lump_data(lump::LIGHTING_HDR)?;
        let (faces, lighting, lighting_is_hdr) = if !hdr_lighting.is_empty() {
            let hdr_faces = lump_data(lump::FACES_HDR)?;
            let faces = if hdr_faces.is_empty() { parse_faces(&lump_data(lump::FACES)?)? } else { parse_faces(&hdr_faces)? };
            (faces, hdr_lighting.to_vec(), true)
        } else {
            (parse_faces(&lump_data(lump::FACES)?)?, lump_data(lump::LIGHTING)?.to_vec(), false)
        };

        let leafs = if version >= 20 || lumps[lump::LEAFS].version >= 1 {
            read_records(&lump_data(lump::LEAFS)?, 32, parse_leaf)?
        } else {
            read_records(&lump_data(lump::LEAFS)?, 56, parse_leaf)?
        };
        let edges = read_records(&lump_data(lump::EDGES)?, 4, |r| Ok([r.u16()?, r.u16()?]))?;
        let surfedges = read_records(&lump_data(lump::SURFEDGES)?, 4, |r| r.i32())?;
        let models = read_records(&lump_data(lump::MODELS)?, 48, |r| {
            Ok(Model {
                mins: r.vec3()?,
                maxs: r.vec3()?,
                origin: r.vec3()?,
                head_node: r.i32()?,
                first_face: r.i32()?,
                num_faces: r.i32()?,
            })
        })?;
        let leaf_faces = read_records(&lump_data(lump::LEAFFACES)?, 2, |r| r.u16())?;
        let leaf_brushes = read_records(&lump_data(lump::LEAFBRUSHES)?, 2, |r| r.u16())?;
        let brushes = read_records(&lump_data(lump::BRUSHES)?, 12, |r| {
            Ok(Brush { first_side: r.i32()?, num_sides: r.i32()?, contents: r.i32()? as u32 })
        })?;
        let brush_sides = read_records(&lump_data(lump::BRUSHSIDES)?, 8, |r| {
            Ok(BrushSide { plane: r.u16()?, texinfo: r.i16()?, dispinfo: r.i16()?, bevel: r.u8()?, thin: r.u8()? })
        })?;
        let disp_infos = read_records(&lump_data(lump::DISPINFO)?, 176, |r| {
            let start_position = r.vec3()?;
            let disp_vert_start = r.i32()?;
            let disp_tri_start = r.i32()?;
            let power = r.i32()?;
            let min_tess = r.i32()?;
            let smoothing_angle = r.f32()?;
            let contents = r.i32()? as u32;
            let map_face = r.u16()?;
            r.skip(2)?;
            let lightmap_alpha_start = r.i32()?;
            let lightmap_sample_position_start = r.i32()?;
            Ok(DispInfo {
                start_position,
                disp_vert_start,
                disp_tri_start,
                power,
                min_tess,
                smoothing_angle,
                contents,
                map_face,
                lightmap_alpha_start,
                lightmap_sample_position_start,
            })
        })?;
        let disp_verts =
            read_records(&lump_data(lump::DISP_VERTS)?, 20, |r| Ok(DispVert { vec: r.vec3()?, dist: r.f32()?, alpha: r.f32()? }))?;
        let disp_tris = read_records(&lump_data(lump::DISP_TRIS)?, 2, |r| r.u16())?;
        let visibility = Visibility::parse(&lump_data(lump::VISIBILITY)?)?;
        let game_lumps = parse_game_lumps(data, lumps[lump::GAME_LUMP])?;
        let static_props =
            game_lumps.iter().find(|g| &g.id == b"sprp").map(|g| parse_static_props(&g.data, g.version)).transpose()?.unwrap_or_default();
        let pakfile = lump_data(lump::PAKFILE)?.to_vec();
        let cubemaps =
            read_records(&lump_data(lump::CUBEMAPS)?, 16, |r| Ok(Cubemap { origin: [r.i32()?, r.i32()?, r.i32()?], size: r.i32()? }))?;
        let overlays = read_records(&lump_data(lump::OVERLAYS)?, 352, |r| {
            let id = r.i32()?;
            let texinfo = r.i16()?;
            let fc = r.u16()?;
            let mut faces = Vec::new();
            for _ in 0..64 {
                faces.push(r.i32()?);
            }
            let face_count = fc & 0x3FFF;
            faces.truncate(face_count as usize);
            Ok(Overlay {
                id,
                texinfo,
                face_count,
                render_order: fc >> 14,
                faces,
                u: [r.f32()?, r.f32()?],
                v: [r.f32()?, r.f32()?],
                uv_points: [r.vec3()?, r.vec3()?, r.vec3()?, r.vec3()?],
                origin: r.vec3()?,
                basis_normal: r.vec3()?,
            })
        })?;

        let (amb_idx_lump, amb_lump) = if lighting_is_hdr {
            (lump::LEAF_AMBIENT_INDEX_HDR, lump::LEAF_AMBIENT_LIGHTING_HDR)
        } else {
            (lump::LEAF_AMBIENT_INDEX, lump::LEAF_AMBIENT_LIGHTING)
        };
        let leaf_ambient_index =
            read_records(&lump_data(amb_idx_lump)?, 4, |r| Ok(LeafAmbientIndex { sample_count: r.u16()?, first_sample: r.u16()? }))?;
        let leaf_ambient_samples = read_records(&lump_data(amb_lump)?, 28, |r| {
            let mut cube = [Vec3::ZERO; 6];
            for c in cube.iter_mut() {
                // Ambient samples are stored without the 1/255 normalisation that
                // lightmap texels carry; measured against floor lightmaps the two
                // only agree once the samples are scaled back up.
                *c = rgbexp_to_linear(r.array()?) * 255.0;
            }
            let pos = [r.u8()?, r.u8()?, r.u8()?];
            Ok(LeafAmbientSample { cube, pos })
        })?;

        let wl_lump = if lighting_is_hdr { lump::WORLDLIGHTS_HDR } else { lump::WORLDLIGHTS };
        let wl_version = lumps[wl_lump].version;
        let wl_stride = if wl_version >= 1 { 100 } else { 88 };
        let world_lights = read_records(&lump_data(wl_lump)?, wl_stride, |r| {
            let origin = r.vec3()?;
            let intensity = r.vec3()?;
            let normal = r.vec3()?;
            if wl_version >= 1 {
                r.skip(12)?; // shadow cast offset
            }
            Ok(WorldLight {
                origin,
                intensity,
                normal,
                cluster: r.i32()?,
                kind: r.i32()?,
                style: r.i32()?,
                stopdot: r.f32()?,
                stopdot2: r.f32()?,
                exponent: r.f32()?,
                radius: r.f32()?,
                constant_attn: r.f32()?,
                linear_attn: r.f32()?,
                quadratic_attn: r.f32()?,
                flags: r.i32()?,
                texinfo: r.i32()?,
                owner: r.i32()?,
            })
        })?;
        let phys_collide = lump_data(lump::PHYSCOLLIDE)?.to_vec();
        let vert_normals = read_records(&lump_data(lump::VERTNORMALS)?, 12, |r| r.vec3())?;
        let vert_normal_indices = read_records(&lump_data(lump::VERTNORMALINDICES)?, 2, |r| r.u16())?;
        let primitives = read_records(&lump_data(lump::PRIMITIVES)?, 10, |r| {
            let kind = r.u8()?;
            r.skip(1)?;
            Ok(Primitive { kind, first_index: r.u16()?, index_count: r.u16()?, first_vert: r.u16()?, vert_count: r.u16()? })
        })?;
        let prim_verts = read_records(&lump_data(lump::PRIMVERTS)?, 12, |r| r.vec3())?;
        let prim_indices = read_records(&lump_data(lump::PRIMINDICES)?, 2, |r| r.u16())?;

        Ok(Self {
            version,
            map_revision,
            lumps,
            entities,
            planes,
            texdata,
            texdata_strings,
            vertices,
            nodes,
            texinfo,
            faces,
            lighting,
            lighting_is_hdr,
            leafs,
            edges,
            surfedges,
            models,
            leaf_faces,
            leaf_brushes,
            brushes,
            brush_sides,
            disp_infos,
            disp_verts,
            disp_tris,
            visibility,
            game_lumps,
            static_props,
            pakfile,
            cubemaps,
            overlays,
            leaf_ambient_index,
            leaf_ambient_samples,
            world_lights,
            phys_collide,
            vert_normals,
            vert_normal_indices,
            primitives,
            prim_verts,
            prim_indices,
        })
    }

    /// Material name of a texinfo, e.g. `metal/black_wall_metal_002a`.
    pub fn texinfo_material(&self, texinfo: usize) -> Option<&str> {
        let ti = self.texinfo.get(texinfo)?;
        let td = self.texdata.get(ti.texdata as usize)?;
        self.texdata_strings.get(td.name_index as usize).map(|s| s.as_str())
    }

    /// Ordered vertex positions of a face polygon.
    pub fn face_vertices(&self, face: &Face) -> Vec<Vec3> {
        let mut out = Vec::with_capacity(face.num_edges.max(0) as usize);
        for i in 0..face.num_edges.max(0) as i32 {
            let se = self.surfedges[(face.first_edge + i) as usize];
            let v = if se >= 0 { self.edges[se as usize][0] } else { self.edges[(-se) as usize][1] };
            out.push(self.vertices[v as usize]);
        }
        out
    }

    /// Finds the leaf containing a point by walking the BSP tree from a head node.
    pub fn find_leaf(&self, point: Vec3) -> usize {
        self.find_leaf_from(0, point)
    }

    pub fn find_leaf_from(&self, head_node: i32, point: Vec3) -> usize {
        let mut node = head_node;
        while node >= 0 {
            let n = &self.nodes[node as usize];
            let p = &self.planes[n.plane as usize];
            let d = p.normal.dot(point) - p.dist;
            node = if d >= 0.0 { n.children[0] } else { n.children[1] };
        }
        (-1 - node) as usize
    }

    /// Point contents from the world BSP (brush contents of the containing leaf).
    pub fn point_contents(&self, point: Vec3) -> u32 {
        self.leafs.get(self.find_leaf(point)).map(|l| l.contents).unwrap_or(0)
    }

    /// Leaf ambient lighting at `point` (linear RGB, 6 directions). Like the
    /// engine, this blends every sample of the containing leaf, each weighted
    /// by 1 / (distance² + 1). A leaf without samples of its own may name
    /// another leaf (in `first_sample`) whose samples it borrows.
    pub fn ambient_cube(&self, point: Vec3) -> Option<[Vec3; 6]> {
        let mut leaf_index = self.find_leaf(point);
        let mut idx = *self.leaf_ambient_index.get(leaf_index)?;
        if idx.sample_count == 0 && idx.first_sample != 0 {
            leaf_index = idx.first_sample as usize;
            idx = *self.leaf_ambient_index.get(leaf_index)?;
        }
        if idx.sample_count == 0 {
            return None;
        }
        let leaf = self.leafs.get(leaf_index)?;
        let mins = Vec3::new(leaf.mins[0] as f32, leaf.mins[1] as f32, leaf.mins[2] as f32);
        let maxs = Vec3::new(leaf.maxs[0] as f32, leaf.maxs[1] as f32, leaf.maxs[2] as f32);
        let first = idx.first_sample as usize;
        let samples = self.leaf_ambient_samples.get(first..first + idx.sample_count as usize)?;
        let mut cube = [Vec3::ZERO; 6];
        let mut total = 0.0;
        for s in samples {
            let t = Vec3::new(s.pos[0] as f32, s.pos[1] as f32, s.pos[2] as f32) / 255.0;
            let p = mins + (maxs - mins) * t;
            let w = 1.0 / (p.distance_squared(point) + 1.0);
            total += w;
            for (dst, src) in cube.iter_mut().zip(s.cube) {
                *dst += src * w;
            }
        }
        Some(cube.map(|c| c / total))
    }

    /// Lightmap samples of a face for one style: `w*h` linear RGB values.
    pub fn face_lightmap(&self, face: &Face, style_index: usize) -> Option<Vec<Vec3>> {
        if face.light_offset < 0 {
            return None;
        }
        let w = (face.lightmap_size[0] + 1) as usize;
        let h = (face.lightmap_size[1] + 1) as usize;
        let start = face.light_offset as usize + style_index * w * h * 4;
        let bytes = self.lighting.get(start..start + w * h * 4)?;
        Some(bytes.as_chunks::<4>().0.iter().map(|c| rgbexp_to_linear([c[0], c[1], c[2], c[3]])).collect())
    }
}

fn parse_leaf(r: &mut Reader) -> Result<Leaf> {
    let contents = r.i32()? as u32;
    let cluster = r.i16()?;
    let area_flags = r.u16()?;
    Ok(Leaf {
        contents,
        cluster,
        area: area_flags & 0x1FF,
        flags: area_flags >> 9,
        mins: [r.i16()?, r.i16()?, r.i16()?],
        maxs: [r.i16()?, r.i16()?, r.i16()?],
        first_leaf_face: r.u16()?,
        num_leaf_faces: r.u16()?,
        first_leaf_brush: r.u16()?,
        num_leaf_brushes: r.u16()?,
        water_data: r.i16()?,
    })
}

fn parse_game_lumps(data: &[u8], info: LumpInfo) -> Result<Vec<GameLump>> {
    if info.length < 4 {
        return Ok(Vec::new());
    }
    let mut r = Reader::at(data, info.offset);
    let count = r.i32()?.max(0) as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let id_raw = r.u32()?;
        let flags = r.u16()?;
        let version = r.u16()?;
        let offset = r.i32()? as usize;
        let length = r.i32()? as usize;
        // Game lump ids are stored as little-endian integers of a big-endian
        // fourcc, so 'sprp' reads back as the bytes "prps".
        let id = id_raw.to_be_bytes();
        let raw = data.get(offset..offset + length).ok_or(Error::Eof)?;
        let bytes = if flags & 1 != 0 && raw.starts_with(b"LZMA") { crate::zip::decode_valve_lzma_header(raw)? } else { raw.to_vec() };
        out.push(GameLump { id, flags, version, data: bytes });
    }
    Ok(out)
}

fn parse_static_props(data: &[u8], version: u16) -> Result<StaticProps> {
    let mut r = Reader::new(data);
    let dict_count = r.i32()?.max(0) as usize;
    let mut models = Vec::with_capacity(dict_count);
    for _ in 0..dict_count {
        models.push(str_from_nul(r.bytes(128)?));
    }
    let leaf_count = r.i32()?.max(0) as usize;
    let mut leaves = Vec::with_capacity(leaf_count);
    for _ in 0..leaf_count {
        leaves.push(r.u16()?);
    }
    let prop_count = r.i32()?.max(0) as usize;
    if prop_count == 0 {
        return Ok(StaticProps { version, models, leaves, props: Vec::new() });
    }
    let stride = r.remaining() / prop_count;
    let base = r.pos();
    let mut props = Vec::with_capacity(prop_count);
    for i in 0..prop_count {
        let mut p = Reader::at(data, base + i * stride);
        let origin = p.vec3()?;
        let angles = p.vec3()?;
        let model = p.u16()?;
        let first_leaf = p.u16()?;
        let leaf_count = p.u16()?;
        let solid = p.u8()?;
        let flags = p.u8()?;
        let skin = p.i32()?;
        let fade_min = p.f32()?;
        let fade_max = p.f32()?;
        let lighting_origin = p.vec3()?;
        let mut forced_fade_scale = 1.0;
        let mut diffuse_modulation = [255u8; 4];
        let mut uniform_scale = 1.0;
        if version >= 5 {
            forced_fade_scale = p.f32()?;
        }
        match version {
            6 | 7 => {
                p.skip(4)?; // min/max dx level
                if version == 7 {
                    diffuse_modulation = p.array()?;
                }
            }
            v if v >= 8 => {
                p.skip(4)?; // cpu/gpu levels
                diffuse_modulation = p.array()?;
                if v >= 9 {
                    p.skip(4)?; // disable x360
                }
                if v >= 10 {
                    p.skip(4)?; // extended flags
                }
                if v >= 11 && stride >= 80 {
                    uniform_scale = p.f32()?;
                }
            }
            _ => {}
        }
        props.push(StaticProp {
            origin,
            angles,
            model,
            first_leaf,
            leaf_count,
            solid,
            flags,
            skin,
            fade_min,
            fade_max,
            lighting_origin,
            forced_fade_scale,
            diffuse_modulation,
            uniform_scale,
        });
    }
    Ok(StaticProps { version, models, leaves, props })
}
