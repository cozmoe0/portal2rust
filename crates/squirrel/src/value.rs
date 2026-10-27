//! Script values and the heap they live on.
//!
//! Reference types (tables, arrays, functions, classes, ...) are stored in an
//! arena and referred to by index plus generation, so the whole heap is plain
//! data: it can be walked by the garbage collector and serialized for saved
//! games. Strings are immutable byte strings shared by reference count.

use std::fmt;
use std::sync::Arc;

use crate::compiler::Proto;
use crate::table::SqTable;

/// Immutable byte string with its precomputed table hash.
#[derive(Clone)]
pub struct SqStr(Arc<StrInner>);

struct StrInner {
    hash: u32,
    bytes: Box<[u8]>,
}

impl SqStr {
    pub fn new(bytes: &[u8]) -> Self {
        Self(Arc::new(StrInner { hash: hash_str(bytes), bytes: bytes.into() }))
    }

    pub fn bytes(&self) -> &[u8] {
        &self.0.bytes
    }

    pub fn hash(&self) -> u32 {
        self.0.hash
    }

    pub fn len(&self) -> usize {
        self.0.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.bytes.is_empty()
    }

    /// The bytes as text, one character per byte (the scripts' encoding).
    pub fn to_string_lossy(&self) -> String {
        self.0.bytes.iter().map(|&b| b as char).collect()
    }
}

impl From<&str> for SqStr {
    fn from(s: &str) -> Self {
        // Characters above 0xFF cannot be represented; they become '?'.
        let bytes: Vec<u8> = s.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect();
        SqStr::new(&bytes)
    }
}

impl PartialEq for SqStr {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || (self.0.hash == other.0.hash && self.0.bytes == other.0.bytes)
    }
}

impl Eq for SqStr {}

impl fmt::Debug for SqStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.to_string_lossy())
    }
}

/// The original's string hash: seeded with the length, mixing a sample of
/// the characters (every `step`th for long strings).
pub fn hash_str(s: &[u8]) -> u32 {
    let mut l = s.len();
    let mut h = l as u32;
    let step = (l >> 5) | 1;
    let mut i = 0;
    while l >= step {
        h ^= (h << 5).wrapping_add(h >> 2).wrapping_add(s[i] as u32);
        i += 1;
        l -= step;
    }
    h
}

/// Reference to an object on the heap.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ObjRef {
    pub index: u32,
    pub generation: u32,
}

#[derive(Clone, Debug, Default)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Int(i32),
    Float(f32),
    Str(SqStr),
    Table(ObjRef),
    Array(ObjRef),
    Closure(ObjRef),
    Native(ObjRef),
    Class(ObjRef),
    Instance(ObjRef),
    Generator(ObjRef),
    WeakRef(ObjRef),
}

impl Value {
    pub fn str(s: &str) -> Self {
        Value::Str(SqStr::from(s))
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Script truthiness: null, false, 0 and 0.0 are false.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::Float(f) => *f != 0.0,
            _ => true,
        }
    }

    pub fn obj(&self) -> Option<ObjRef> {
        match self {
            Value::Table(r)
            | Value::Array(r)
            | Value::Closure(r)
            | Value::Native(r)
            | Value::Class(r)
            | Value::Instance(r)
            | Value::Generator(r)
            | Value::WeakRef(r) => Some(*r),
            _ => None,
        }
    }

    /// Same type and same raw value (string contents, object identity).
    pub fn raw_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (a, b) => std::mem::discriminant(a) == std::mem::discriminant(b) && a.obj() == b.obj(),
        }
    }

    /// Numeric value of integers and floats.
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Value::Int(i) => Some(*i as f32),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f) => Some(*f as i32),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&SqStr> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "integer",
            Value::Float(_) => "float",
            Value::Str(_) => "string",
            Value::Table(_) => "table",
            Value::Array(_) => "array",
            Value::Closure(_) => "function",
            Value::Native(_) => "native function",
            Value::Class(_) => "class",
            Value::Instance(_) => "instance",
            Value::Generator(_) => "generator",
            Value::WeakRef(_) => "weakref",
        }
    }
}

/// Native function: receives `this` followed by the arguments.
pub type NativeFn = fn(&mut crate::vm::Ctx, &[Value]) -> crate::error::Result<Value>;

#[derive(Clone)]
pub struct NativeClosure {
    pub name: SqStr,
    pub func: NativeFn,
    /// >0: exact argument count including `this`; <0: at least `-n`; 0: any.
    pub nparams: i32,
    /// Value bound when the native was created (available to `func`).
    pub data: Value,
    pub env: Option<Value>,
}

#[derive(Clone)]
pub struct Closure {
    pub proto: Arc<Proto>,
    /// Captured copies of enclosing locals (by value, as in Squirrel 2).
    pub outers: Vec<Value>,
    /// Default parameter values, evaluated when the closure was created.
    pub defaults: Vec<Value>,
    /// Fixed `this` set by `bindenv` (held weakly).
    pub env: Option<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberKind {
    /// Per-instance slot with a default value.
    Field,
    /// Shared by all instances (functions and statics).
    Method,
}

#[derive(Clone)]
pub struct Class {
    pub base: Option<ObjRef>,
    /// Member name -> (kind, index into `fields` or `methods`), in
    /// declaration order.
    pub members: Vec<(Value, MemberKind, usize)>,
    /// (default value, attributes) per field.
    pub fields: Vec<(Value, Value)>,
    /// (function or static value, attributes) per method.
    pub methods: Vec<(Value, Value)>,
    /// Metamethods (`_get`, `_add`, ...) by name; not visible as members.
    pub metamethods: Vec<(Vec<u8>, Value)>,
    pub attrs: Value,
    pub locked: bool,
    /// Host type tag for native classes (entity handles etc.).
    pub type_tag: u32,
}

impl Class {
    pub fn find(&self, key: &Value) -> Option<(MemberKind, usize)> {
        self.members.iter().find(|(k, _, _)| k.raw_eq(key)).map(|(_, kind, i)| (*kind, *i))
    }

    pub fn metamethod(&self, name: &[u8]) -> Option<&Value> {
        self.metamethods.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
}

/// Names that make a class member a metamethod.
pub const METAMETHODS: &[&[u8]] = &[
    b"_add", b"_sub", b"_mul", b"_div", b"_unm", b"_modulo", b"_set", b"_get", b"_typeof", b"_nexti", b"_cmp", b"_call",
    b"_cloned", b"_newslot", b"_delslot", b"_tostring", b"_newmember", b"_inherited",
];

#[derive(Clone)]
pub struct Instance {
    pub class: ObjRef,
    pub fields: Vec<Value>,
    /// Host data (an entity id for entity handles).
    pub user: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenState {
    Suspended,
    Running,
    Dead,
}

#[derive(Clone)]
pub struct Generator {
    pub state: GenState,
    pub closure: ObjRef,
    /// Saved frame slots (this, locals and temporaries) while suspended.
    pub stack: Vec<Value>,
    pub ip: usize,
    pub varargs: Vec<Value>,
    pub this: Value,
}

pub struct Table {
    pub map: SqTable,
    pub delegate: Option<ObjRef>,
}

pub enum Object {
    Table(Table),
    Array(Vec<Value>),
    Closure(Closure),
    Native(NativeClosure),
    Class(Class),
    Instance(Instance),
    Generator(Generator),
    /// Weak reference to the value (objects are not kept alive by it).
    WeakRef(Value),
}

struct Slot {
    generation: u32,
    marked: bool,
    obj: Option<Object>,
}

/// Arena of objects with mark-and-sweep collection.
#[derive(Default)]
pub struct Heap {
    slots: Vec<Slot>,
    free: Vec<u32>,
    pub allocated_since_collect: usize,
}

impl Heap {
    pub fn alloc(&mut self, obj: Object) -> ObjRef {
        self.allocated_since_collect += 1;
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.obj = Some(obj);
            return ObjRef { index, generation: slot.generation };
        }
        self.slots.push(Slot { generation: 0, marked: false, obj: Some(obj) });
        ObjRef { index: (self.slots.len() - 1) as u32, generation: 0 }
    }

    pub fn is_alive(&self, r: ObjRef) -> bool {
        self.slots.get(r.index as usize).is_some_and(|s| s.generation == r.generation && s.obj.is_some())
    }

    pub fn get(&self, r: ObjRef) -> &Object {
        match self.slots.get(r.index as usize) {
            Some(s) if s.generation == r.generation => s.obj.as_ref().expect("live object"),
            _ => panic!("stale object reference"),
        }
    }

    pub fn get_mut(&mut self, r: ObjRef) -> &mut Object {
        match self.slots.get_mut(r.index as usize) {
            Some(s) if s.generation == r.generation => s.obj.as_mut().expect("live object"),
            _ => panic!("stale object reference"),
        }
    }

    pub fn table(&self, r: ObjRef) -> &Table {
        match self.get(r) {
            Object::Table(t) => t,
            _ => panic!("not a table"),
        }
    }

    pub fn table_mut(&mut self, r: ObjRef) -> &mut Table {
        match self.get_mut(r) {
            Object::Table(t) => t,
            _ => panic!("not a table"),
        }
    }

    pub fn array(&self, r: ObjRef) -> &Vec<Value> {
        match self.get(r) {
            Object::Array(a) => a,
            _ => panic!("not an array"),
        }
    }

    pub fn array_mut(&mut self, r: ObjRef) -> &mut Vec<Value> {
        match self.get_mut(r) {
            Object::Array(a) => a,
            _ => panic!("not an array"),
        }
    }

    pub fn class(&self, r: ObjRef) -> &Class {
        match self.get(r) {
            Object::Class(c) => c,
            _ => panic!("not a class"),
        }
    }

    pub fn class_mut(&mut self, r: ObjRef) -> &mut Class {
        match self.get_mut(r) {
            Object::Class(c) => c,
            _ => panic!("not a class"),
        }
    }

    pub fn instance(&self, r: ObjRef) -> &Instance {
        match self.get(r) {
            Object::Instance(i) => i,
            _ => panic!("not an instance"),
        }
    }

    pub fn instance_mut(&mut self, r: ObjRef) -> &mut Instance {
        match self.get_mut(r) {
            Object::Instance(i) => i,
            _ => panic!("not an instance"),
        }
    }

    pub fn closure(&self, r: ObjRef) -> &Closure {
        match self.get(r) {
            Object::Closure(c) => c,
            _ => panic!("not a closure"),
        }
    }

    pub fn native(&self, r: ObjRef) -> &NativeClosure {
        match self.get(r) {
            Object::Native(n) => n,
            _ => panic!("not a native"),
        }
    }

    pub fn generator_mut(&mut self, r: ObjRef) -> &mut Generator {
        match self.get_mut(r) {
            Object::Generator(g) => g,
            _ => panic!("not a generator"),
        }
    }

    /// Target of a weak reference, or null once it has been collected.
    pub fn weak_target(&self, r: ObjRef) -> Value {
        match self.get(r) {
            Object::WeakRef(v) => match v.obj() {
                Some(o) if !self.is_alive(o) => Value::Null,
                _ => v.clone(),
            },
            _ => Value::Null,
        }
    }

    pub fn live_objects(&self) -> usize {
        self.slots.iter().filter(|s| s.obj.is_some()).count()
    }

    /// Frees every object not reachable from `roots`.
    pub fn collect<'a>(&mut self, roots: impl IntoIterator<Item = &'a Value>) {
        let mut stack: Vec<ObjRef> = roots.into_iter().filter_map(Value::obj).collect();
        while let Some(r) = stack.pop() {
            let Some(slot) = self.slots.get_mut(r.index as usize) else { continue };
            if slot.generation != r.generation || slot.marked || slot.obj.is_none() {
                continue;
            }
            slot.marked = true;
            let mut children: Vec<ObjRef> = Vec::new();
            {
                let mut push = |v: &Value| {
                    if let Some(o) = v.obj() {
                        children.push(o);
                    }
                };
                match slot.obj.as_ref().expect("checked") {
                    Object::Table(t) => {
                        for (k, v) in t.map.iter() {
                            push(k);
                            push(v);
                        }
                        if let Some(d) = t.delegate {
                            push(&Value::Table(d));
                        }
                    }
                    Object::Array(a) => a.iter().for_each(&mut push),
                    Object::Closure(c) => {
                        c.outers.iter().for_each(&mut push);
                        c.defaults.iter().for_each(&mut push);
                        c.proto.consts.iter().for_each(&mut push);
                        if let Some(e) = &c.env {
                            push(e);
                        }
                    }
                    Object::Native(n) => {
                        push(&n.data);
                        if let Some(e) = &n.env {
                            push(e);
                        }
                    }
                    Object::Class(c) => {
                        if let Some(b) = c.base {
                            push(&Value::Class(b));
                        }
                        for (k, _, _) in &c.members {
                            push(k);
                        }
                        for (v, a) in c.fields.iter().chain(&c.methods) {
                            push(v);
                            push(a);
                        }
                        for (_, m) in &c.metamethods {
                            push(m);
                        }
                        push(&c.attrs);
                    }
                    Object::Instance(i) => {
                        push(&Value::Class(i.class));
                        i.fields.iter().for_each(&mut push);
                    }
                    Object::Generator(g) => {
                        push(&Value::Closure(g.closure));
                        g.stack.iter().for_each(&mut push);
                        g.varargs.iter().for_each(&mut push);
                        push(&g.this);
                    }
                    Object::WeakRef(_) => {}
                }
            }
            stack.extend(children);
        }
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.obj.is_some() && !slot.marked {
                slot.obj = None;
                slot.generation = slot.generation.wrapping_add(1);
                self.free.push(i as u32);
            }
            slot.marked = false;
        }
        self.allocated_since_collect = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_hash_matches_reference_values() {
        // Seed is the length; empty strings hash to 0.
        assert_eq!(hash_str(b""), 0);
        let h1 = hash_str(b"a");
        assert_eq!(h1, 1 ^ ((1u32 << 5).wrapping_add(0).wrapping_add(97)));
    }
}
