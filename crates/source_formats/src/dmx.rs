//! Binary DMX reader: the element / attribute trees written by Valve's
//! datamodel (`<!-- dmx encoding binary N format F M -->`), used by particle
//! definitions (`.pcf`).
//!
//! Binary encodings 1-5 are understood; Portal 2 ships 2, 3 and 5. Text
//! (`keyvalues2`) encodings are not. See `docs/re/pcf.md` for the layout.

use std::fmt;

use glam::{Mat4, Quat, Vec2, Vec3, Vec4};

use crate::reader::Reader;
use crate::{Error, Result, invalid};

const MAGIC: &[u8] = b"<!-- dmx encoding ";

/// DMX time values are integer ticks of 1/10000 s.
const TIME_TICKS_PER_SECOND: f32 = 10_000.0;

/// The `<!-- dmx encoding ... format ... -->` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub encoding: String,
    pub encoding_version: u32,
    pub format: String,
    pub format_version: u32,
}

#[derive(Clone, Debug)]
pub struct Dmx {
    pub header: Header,
    /// Every element in file order; element 0 is the root.
    pub elements: Vec<Element>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    /// Element class, e.g. `DmeParticleSystemDefinition`.
    pub type_name: String,
    pub name: String,
    /// GUID as stored.
    pub id: [u8; 16],
    pub attributes: Vec<Attribute>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub value: Value,
}

/// Attribute value. Element references are indices into [`Dmx::elements`],
/// `None` for a null reference.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Element(Option<usize>),
    Int(i32),
    Float(f32),
    Bool(bool),
    String(String),
    Binary(Vec<u8>),
    /// Seconds.
    Time(f32),
    /// RGBA.
    Color([u8; 4]),
    Vector2(Vec2),
    Vector3(Vec3),
    Vector4(Vec4),
    /// Pitch, yaw, roll in degrees.
    QAngle(Vec3),
    Quaternion(Quat),
    Matrix(Mat4),
    ElementArray(Vec<Option<usize>>),
    IntArray(Vec<i32>),
    FloatArray(Vec<f32>),
    BoolArray(Vec<bool>),
    StringArray(Vec<String>),
    BinaryArray(Vec<Vec<u8>>),
    TimeArray(Vec<f32>),
    ColorArray(Vec<[u8; 4]>),
    Vector2Array(Vec<Vec2>),
    Vector3Array(Vec<Vec3>),
    Vector4Array(Vec<Vec4>),
    QAngleArray(Vec<Vec3>),
    QuaternionArray(Vec<Quat>),
    MatrixArray(Vec<Mat4>),
}

impl Dmx {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let (header, body) = parse_header(data)?;
        if header.encoding != "binary" {
            return invalid(format!("unsupported dmx encoding {}", header.encoding));
        }
        let version = header.encoding_version;
        if !(1..=5).contains(&version) {
            return Err(Error::Version(version as i64));
        }
        let mut r = Reader::at(data, body);
        let strings = Strings::read(&mut r, version)?;

        let count = read_count(&mut r, 16)?;
        let mut elements = Vec::with_capacity(count);
        for _ in 0..count {
            let type_name = strings.symbol(&mut r)?;
            let name = strings.text(&mut r)?;
            let id = r.array()?;
            elements.push(Element { type_name, name, id, attributes: Vec::new() });
        }
        for element in &mut elements {
            let n = read_count(&mut r, 2)?;
            let mut attributes = Vec::with_capacity(n);
            for _ in 0..n {
                let name = strings.symbol(&mut r)?;
                let kind = r.u8()?;
                let value = read_value(&mut r, &strings, kind, count)?;
                attributes.push(Attribute { name, value });
            }
            element.attributes = attributes;
        }
        if r.remaining() > 0 {
            log::warn!("{} trailing bytes after dmx elements", r.remaining());
        }
        Ok(Self { header, elements })
    }

    pub fn root(&self) -> Option<&Element> {
        self.elements.first()
    }
}

impl Element {
    /// Attribute value by exact name.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.attributes.iter().find(|a| a.name == name).map(|a| &a.value)
    }
}

/// Splits off the text header; returns it and the offset of the binary body.
pub fn parse_header(data: &[u8]) -> Result<(Header, usize)> {
    if !data.starts_with(MAGIC) {
        return Err(Error::Magic { expected: "<!-- dmx encoding", found: data[..data.len().min(MAGIC.len())].to_vec() });
    }
    let close = data.windows(3).take(256).position(|w| w == b"-->").ok_or_else(|| Error::Invalid("unterminated dmx header".into()))?;
    let text = String::from_utf8_lossy(&data[MAGIC.len()..close]);
    let bad = || Error::Invalid(format!("bad dmx header {text:?}"));
    let words: Vec<&str> = text.split_whitespace().collect();
    let &[encoding, encoding_version, "format", format, format_version] = &words[..] else { return Err(bad()) };
    let header = Header {
        encoding: encoding.to_owned(),
        encoding_version: encoding_version.parse().map_err(|_| bad())?,
        format: format.to_owned(),
        format_version: format_version.parse().map_err(|_| bad())?,
    };
    // The text line ends with a newline and a NUL; the binary body follows.
    let mut pos = close + 3;
    while data.get(pos).is_some_and(u8::is_ascii_whitespace) {
        pos += 1;
    }
    if data.get(pos) != Some(&0) {
        return invalid("dmx header is not NUL terminated");
    }
    Ok((header, pos + 1))
}

/// String table shared by element headers and attributes. Encoding 1 has
/// none; 2+ store type and attribute names as indices; 4+ also element names
/// and (non-array) string values; 5 widens indices to 32 bits.
struct Strings {
    version: u32,
    table: Vec<String>,
}

impl Strings {
    fn read(r: &mut Reader, version: u32) -> Result<Self> {
        let mut table = Vec::new();
        if version >= 2 {
            let count = if version >= 4 { read_count(r, 1)? } else { r.u16()? as usize };
            if count > r.remaining() {
                return Err(Error::Eof);
            }
            table.reserve(count);
            for _ in 0..count {
                table.push(r.cstr()?);
            }
        }
        Ok(Self { version, table })
    }

    /// Element type or attribute name.
    fn symbol(&self, r: &mut Reader) -> Result<String> {
        if self.version >= 2 { self.lookup(r) } else { r.cstr() }
    }

    /// Element name or string attribute value.
    fn text(&self, r: &mut Reader) -> Result<String> {
        if self.version >= 4 { self.lookup(r) } else { r.cstr() }
    }

    fn lookup(&self, r: &mut Reader) -> Result<String> {
        let index = if self.version >= 5 { r.i32()? as i64 } else { r.u16()? as i64 };
        usize::try_from(index)
            .ok()
            .and_then(|i| self.table.get(i))
            .cloned()
            .ok_or_else(|| Error::Invalid(format!("dmx string index {index} out of range")))
    }
}

/// Reads a non-negative count and checks that `min_size` bytes per item remain.
fn read_count(r: &mut Reader, min_size: usize) -> Result<usize> {
    let n = r.i32()?;
    let n = usize::try_from(n).map_err(|_| Error::Invalid(format!("negative dmx count {n}")))?;
    if n.saturating_mul(min_size) > r.remaining() {
        return Err(Error::Eof);
    }
    Ok(n)
}

/// Type ids 1-14 are scalars; 15-28 are arrays of the same types in the same order.
fn read_value(r: &mut Reader, strings: &Strings, kind: u8, elements: usize) -> Result<Value> {
    let element = |r: &mut Reader| -> Result<Option<usize>> {
        match r.i32()? {
            -1 => Ok(None),
            // -2 is followed by the GUID of an element in another file; PCFs never use it.
            -2 => invalid("external dmx element references are not supported"),
            i => match usize::try_from(i) {
                Ok(i) if i < elements => Ok(Some(i)),
                _ => invalid(format!("dmx element index {i} out of range")),
            },
        }
    };
    Ok(match kind {
        1 => Value::Element(element(r)?),
        2 => Value::Int(r.i32()?),
        3 => Value::Float(r.f32()?),
        4 => Value::Bool(r.u8()? != 0),
        5 => Value::String(strings.text(r)?),
        6 => Value::Binary(binary(r)?),
        7 => Value::Time(time(r)?),
        8 => Value::Color(r.array()?),
        9 => Value::Vector2(r.vec2()?),
        10 => Value::Vector3(r.vec3()?),
        11 => Value::Vector4(vec4(r)?),
        12 => Value::QAngle(r.vec3()?),
        13 => Value::Quaternion(r.quat()?),
        14 => Value::Matrix(matrix(r)?),
        15 => Value::ElementArray(array(r, 4, element)?),
        16 => Value::IntArray(array(r, 4, Reader::i32)?),
        17 => Value::FloatArray(array(r, 4, Reader::f32)?),
        18 => Value::BoolArray(array(r, 1, |r| Ok(r.u8()? != 0))?),
        // Array strings are always inline, even when the file has a string table.
        19 => Value::StringArray(array(r, 1, Reader::cstr)?),
        20 => Value::BinaryArray(array(r, 4, binary)?),
        21 => Value::TimeArray(array(r, 4, time)?),
        22 => Value::ColorArray(array(r, 4, Reader::array::<4>)?),
        23 => Value::Vector2Array(array(r, 8, Reader::vec2)?),
        24 => Value::Vector3Array(array(r, 12, Reader::vec3)?),
        25 => Value::Vector4Array(array(r, 16, vec4)?),
        26 => Value::QAngleArray(array(r, 12, Reader::vec3)?),
        27 => Value::QuaternionArray(array(r, 16, Reader::quat)?),
        28 => Value::MatrixArray(array(r, 64, matrix)?),
        _ => return invalid(format!("unknown dmx attribute type {kind}")),
    })
}

fn array<'a, T>(r: &mut Reader<'a>, min_size: usize, mut read: impl FnMut(&mut Reader<'a>) -> Result<T>) -> Result<Vec<T>> {
    let n = read_count(r, min_size)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(read(r)?);
    }
    Ok(out)
}

fn binary(r: &mut Reader) -> Result<Vec<u8>> {
    let n = read_count(r, 1)?;
    Ok(r.bytes(n)?.to_vec())
}

fn time(r: &mut Reader) -> Result<f32> {
    Ok(r.i32()? as f32 / TIME_TICKS_PER_SECOND)
}

fn vec4(r: &mut Reader) -> Result<Vec4> {
    Ok(Vec4::new(r.f32()?, r.f32()?, r.f32()?, r.f32()?))
}

/// Stored row by row (`m[row][col]`, translation in the last column).
fn matrix(r: &mut Reader) -> Result<Mat4> {
    let mut m = [0.0; 16];
    for v in &mut m {
        *v = r.f32()?;
    }
    Ok(Mat4::from_cols_array(&m).transpose())
}

impl Value {
    /// Integer, float or bool value as an integer.
    pub fn as_i32(&self) -> Option<i32> {
        match *self {
            Value::Int(v) => Some(v),
            Value::Float(v) => Some(v as i32),
            Value::Bool(v) => Some(v as i32),
            _ => None,
        }
    }

    /// Float, integer, bool or time value as a float.
    pub fn as_f32(&self) -> Option<f32> {
        match *self {
            Value::Float(v) | Value::Time(v) => Some(v),
            Value::Int(v) => Some(v as f32),
            Value::Bool(v) => Some(v as i32 as f32),
            _ => None,
        }
    }

    /// Bool, integer or float value as a bool (non-zero is true).
    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Value::Bool(v) => Some(v),
            Value::Int(v) => Some(v != 0),
            Value::Float(v) => Some(v != 0.0),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// Vector or angle value.
    pub fn as_vec3(&self) -> Option<Vec3> {
        match *self {
            Value::Vector3(v) | Value::QAngle(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_color(&self) -> Option<[u8; 4]> {
        match *self {
            Value::Color(c) => Some(c),
            _ => None,
        }
    }

    /// Referenced element index; `None` for null references and other types.
    pub fn as_element(&self) -> Option<usize> {
        match *self {
            Value::Element(e) => e,
            _ => None,
        }
    }

    pub fn as_elements(&self) -> Option<&[Option<usize>]> {
        match self {
            Value::ElementArray(v) => Some(v),
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Element(e) => fmt_ref(f, e),
            Value::Int(v) => write!(f, "{v}"),
            Value::Float(v) | Value::Time(v) => write!(f, "{v}"),
            Value::Bool(v) => write!(f, "{v}"),
            Value::String(s) => write!(f, "{s:?}"),
            Value::Binary(b) => write!(f, "<{} bytes>", b.len()),
            Value::Color(c) => fmt_floats(f, c.map(f32::from)),
            Value::Vector2(v) => fmt_floats(f, v.to_array()),
            Value::Vector3(v) | Value::QAngle(v) => fmt_floats(f, v.to_array()),
            Value::Vector4(v) => fmt_floats(f, v.to_array()),
            Value::Quaternion(q) => fmt_floats(f, q.to_array()),
            Value::Matrix(m) => fmt_floats(f, m.transpose().to_cols_array()),
            Value::ElementArray(v) => fmt_list(f, v, fmt_ref),
            Value::IntArray(v) => fmt_list(f, v, |f, x| write!(f, "{x}")),
            Value::FloatArray(v) | Value::TimeArray(v) => fmt_list(f, v, |f, x| write!(f, "{x}")),
            Value::BoolArray(v) => fmt_list(f, v, |f, x| write!(f, "{x}")),
            Value::StringArray(v) => fmt_list(f, v, |f, x| write!(f, "{x:?}")),
            Value::BinaryArray(v) => fmt_list(f, v, |f, x| write!(f, "<{} bytes>", x.len())),
            Value::ColorArray(v) => fmt_list(f, v, |f, x| fmt_floats(f, x.map(f32::from))),
            Value::Vector2Array(v) => fmt_list(f, v, |f, x| fmt_floats(f, x.to_array())),
            Value::Vector3Array(v) | Value::QAngleArray(v) => fmt_list(f, v, |f, x| fmt_floats(f, x.to_array())),
            Value::Vector4Array(v) => fmt_list(f, v, |f, x| fmt_floats(f, x.to_array())),
            Value::QuaternionArray(v) => fmt_list(f, v, |f, x| fmt_floats(f, x.to_array())),
            Value::MatrixArray(v) => fmt_list(f, v, |f, x| fmt_floats(f, x.transpose().to_cols_array())),
        }
    }
}

fn fmt_ref(f: &mut fmt::Formatter<'_>, e: &Option<usize>) -> fmt::Result {
    match e {
        Some(i) => write!(f, "#{i}"),
        None => f.write_str("null"),
    }
}

fn fmt_floats<const N: usize>(f: &mut fmt::Formatter<'_>, v: [f32; N]) -> fmt::Result {
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            f.write_str(" ")?;
        }
        write!(f, "{x}")?;
    }
    Ok(())
}

fn fmt_list<T>(f: &mut fmt::Formatter<'_>, items: &[T], item: impl Fn(&mut fmt::Formatter<'_>, &T) -> fmt::Result) -> fmt::Result {
    f.write_str("[")?;
    for (i, x) in items.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        item(f, x)?;
    }
    f.write_str("]")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes the binary layouts described above, interning strings as it goes.
    struct Writer {
        version: u32,
        table: Vec<String>,
        body: Vec<u8>,
    }

    impl Writer {
        fn new(version: u32) -> Self {
            Self { version, table: Vec::new(), body: Vec::new() }
        }

        fn cstr(&mut self, s: &str) {
            self.body.extend_from_slice(s.as_bytes());
            self.body.push(0);
        }

        fn index(&mut self, s: &str) {
            let i = match self.table.iter().position(|t| t == s) {
                Some(i) => i,
                None => {
                    self.table.push(s.to_owned());
                    self.table.len() - 1
                }
            };
            if self.version >= 5 {
                self.i32(i as i32);
            } else {
                self.body.extend_from_slice(&(i as u16).to_le_bytes());
            }
        }

        fn symbol(&mut self, s: &str) {
            if self.version >= 2 { self.index(s) } else { self.cstr(s) }
        }

        fn text(&mut self, s: &str) {
            if self.version >= 4 { self.index(s) } else { self.cstr(s) }
        }

        fn i32(&mut self, v: i32) {
            self.body.extend_from_slice(&v.to_le_bytes());
        }

        fn floats(&mut self, v: &[f32]) {
            for x in v {
                self.body.extend_from_slice(&x.to_le_bytes());
            }
        }

        fn attr(&mut self, name: &str, kind: u8) {
            self.symbol(name);
            self.body.push(kind);
        }

        fn finish(self) -> Vec<u8> {
            let mut out = format!("<!-- dmx encoding binary {} format pcf 2 -->\n", self.version).into_bytes();
            out.push(0);
            if self.version >= 2 {
                if self.version >= 4 {
                    out.extend_from_slice(&(self.table.len() as i32).to_le_bytes());
                } else {
                    out.extend_from_slice(&(self.table.len() as u16).to_le_bytes());
                }
                for s in &self.table {
                    out.extend_from_slice(s.as_bytes());
                    out.push(0);
                }
            }
            out.extend_from_slice(&self.body);
            out
        }
    }

    /// Two elements: a root carrying one attribute of every type, and a target
    /// for its element references.
    fn sample(version: u32) -> Vec<u8> {
        let mut w = Writer::new(version);
        w.i32(2);
        for (ty, name, id) in [("DmElement", "root", 1u8), ("DmeThing", "thing", 2)] {
            w.symbol(ty);
            w.text(name);
            w.body.extend_from_slice(&[id; 16]);
        }
        let rows = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0)).transpose().to_cols_array();
        w.i32(28);
        w.attr("element", 1);
        w.i32(1);
        w.attr("int", 2);
        w.i32(-7);
        w.attr("float", 3);
        w.floats(&[0.5]);
        w.attr("bool", 4);
        w.body.push(1);
        w.attr("string", 5);
        w.text("effects/a.vmt");
        w.attr("binary", 6);
        w.i32(3);
        w.body.extend_from_slice(&[9, 8, 7]);
        w.attr("time", 7);
        w.i32(25_000);
        w.attr("color", 8);
        w.body.extend_from_slice(&[1, 2, 3, 4]);
        w.attr("vector2", 9);
        w.floats(&[1.0, 2.0]);
        w.attr("vector3", 10);
        w.floats(&[1.0, 2.0, 3.0]);
        w.attr("vector4", 11);
        w.floats(&[1.0, 2.0, 3.0, 4.0]);
        w.attr("qangle", 12);
        w.floats(&[10.0, 20.0, 30.0]);
        w.attr("quaternion", 13);
        w.floats(&[0.0, 0.0, 0.0, 1.0]);
        w.attr("matrix", 14);
        w.floats(&rows);
        w.attr("elements", 15);
        w.i32(2);
        w.i32(-1);
        w.i32(1);
        w.attr("ints", 16);
        w.i32(2);
        w.i32(5);
        w.i32(6);
        w.attr("floats", 17);
        w.i32(1);
        w.floats(&[1.5]);
        w.attr("bools", 18);
        w.i32(2);
        w.body.extend_from_slice(&[0, 1]);
        w.attr("strings", 19);
        w.i32(2);
        w.cstr("a");
        w.cstr("");
        w.attr("binaries", 20);
        w.i32(1);
        w.i32(1);
        w.body.push(42);
        w.attr("times", 21);
        w.i32(1);
        w.i32(5_000);
        w.attr("colors", 22);
        w.i32(1);
        w.body.extend_from_slice(&[5, 6, 7, 8]);
        w.attr("vector2s", 23);
        w.i32(1);
        w.floats(&[3.0, 4.0]);
        w.attr("vector3s", 24);
        w.i32(1);
        w.floats(&[4.0, 5.0, 6.0]);
        w.attr("vector4s", 25);
        w.i32(1);
        w.floats(&[5.0, 6.0, 7.0, 8.0]);
        w.attr("qangles", 26);
        w.i32(1);
        w.floats(&[1.0, 0.0, 0.0]);
        w.attr("quaternions", 27);
        w.i32(1);
        w.floats(&[0.0, 1.0, 0.0, 0.0]);
        w.attr("matrices", 28);
        w.i32(1);
        w.floats(&rows);
        w.i32(0);
        w.finish()
    }

    #[test]
    fn every_encoding_and_type() {
        for version in 1..=5 {
            let dmx = Dmx::parse(&sample(version)).unwrap();
            assert_eq!(dmx.header.encoding_version, version);
            assert_eq!(dmx.header.format, "pcf");
            assert_eq!(dmx.elements.len(), 2);
            let root = dmx.root().unwrap();
            assert_eq!((root.type_name.as_str(), root.name.as_str(), root.id), ("DmElement", "root", [1; 16]));
            assert_eq!(dmx.elements[1].name, "thing");
            assert_eq!(root.attributes.len(), 28);
            let get = |name: &str| root.get(name).unwrap().clone();
            assert_eq!(get("element").as_element(), Some(1));
            assert_eq!(get("int").as_i32(), Some(-7));
            assert_eq!(get("float").as_f32(), Some(0.5));
            assert_eq!(get("bool").as_bool(), Some(true));
            assert_eq!(get("string").as_str(), Some("effects/a.vmt"));
            assert_eq!(get("binary"), Value::Binary(vec![9, 8, 7]));
            assert_eq!(get("time"), Value::Time(2.5));
            assert_eq!(get("color").as_color(), Some([1, 2, 3, 4]));
            assert_eq!(get("vector2"), Value::Vector2(Vec2::new(1.0, 2.0)));
            assert_eq!(get("vector3").as_vec3(), Some(Vec3::new(1.0, 2.0, 3.0)));
            assert_eq!(get("vector4"), Value::Vector4(Vec4::new(1.0, 2.0, 3.0, 4.0)));
            assert_eq!(get("qangle").as_vec3(), Some(Vec3::new(10.0, 20.0, 30.0)));
            assert_eq!(get("quaternion"), Value::Quaternion(Quat::IDENTITY));
            assert_eq!(get("matrix"), Value::Matrix(Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0))));
            assert_eq!(get("elements").as_elements(), Some(&[None, Some(1)][..]));
            assert_eq!(get("ints"), Value::IntArray(vec![5, 6]));
            assert_eq!(get("floats"), Value::FloatArray(vec![1.5]));
            assert_eq!(get("bools"), Value::BoolArray(vec![false, true]));
            assert_eq!(get("strings"), Value::StringArray(vec!["a".into(), String::new()]));
            assert_eq!(get("binaries"), Value::BinaryArray(vec![vec![42]]));
            assert_eq!(get("times"), Value::TimeArray(vec![0.5]));
            assert_eq!(get("colors"), Value::ColorArray(vec![[5, 6, 7, 8]]));
            assert_eq!(get("vector2s"), Value::Vector2Array(vec![Vec2::new(3.0, 4.0)]));
            assert_eq!(get("vector3s"), Value::Vector3Array(vec![Vec3::new(4.0, 5.0, 6.0)]));
            assert_eq!(get("vector4s"), Value::Vector4Array(vec![Vec4::new(5.0, 6.0, 7.0, 8.0)]));
            assert_eq!(get("qangles"), Value::QAngleArray(vec![Vec3::X]));
            assert_eq!(get("quaternions"), Value::QuaternionArray(vec![Quat::from_xyzw(0.0, 1.0, 0.0, 0.0)]));
            assert_eq!(get("matrices"), Value::MatrixArray(vec![Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0))]));
            assert_eq!(get("color").to_string(), "1 2 3 4");
            assert_eq!(get("elements").to_string(), "[null, #1]");
        }
    }

    #[test]
    fn rejects_bad_input() {
        let data = sample(5);
        for cut in [10, 60, data.len() / 2, data.len() - 1] {
            assert!(Dmx::parse(&data[..cut]).is_err(), "truncated at {cut}");
        }
        assert!(matches!(Dmx::parse(b"<!-- DMXVersion binary_v2 -->\n\0"), Err(Error::Magic { .. })));
        let text = b"<!-- dmx encoding keyvalues2 1 format pcf 1 -->\n\0";
        assert!(matches!(Dmx::parse(text), Err(Error::Invalid(_))));
        let future = b"<!-- dmx encoding binary 9 format pcf 1 -->\n\0";
        assert!(matches!(Dmx::parse(future), Err(Error::Version(9))));
    }
}
