//! Saved-game snapshots of a VM.
//!
//! The heap is stored slot by slot with its generations and free list, so
//! object identities, table layouts (which hash objects by slot) and future
//! allocations come back exactly. Strings and compiled functions are stored
//! once each and referred to by index. Native functions are stored by a key,
//! their name and their place among the natives of that name in the order
//! the VM made them, and are looked up again in the VM being restored into,
//! which the host must have set up the same way. Host data in instances
//! (entity handles) passes through a host mapping on both sides.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::compiler::{Op, OuterSource, Proto};
use crate::table::{Node, SqTable};
use crate::value::*;
use crate::vm::{Delegates, Vm};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Bytes {
    Text(String),
    Raw(Vec<u8>),
}

impl Bytes {
    fn of(b: &[u8]) -> Self {
        match std::str::from_utf8(b) {
            Ok(s) => Bytes::Text(s.to_owned()),
            Err(_) => Bytes::Raw(b.to_vec()),
        }
    }

    fn bytes(&self) -> &[u8] {
        match self {
            Bytes::Text(s) => s.as_bytes(),
            Bytes::Raw(b) => b,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ValueSnap {
    Null,
    Bool(bool),
    Int(i32),
    Float(f32),
    /// Index into [`VmSnapshot::strings`].
    Str(u32),
    Table(ObjRef),
    Array(ObjRef),
    Closure(ObjRef),
    Native(ObjRef),
    Class(ObjRef),
    Instance(ObjRef),
    Generator(ObjRef),
    WeakRef(ObjRef),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProtoSnap {
    pub name: u32,
    pub source: u32,
    pub params: Vec<u32>,
    pub ndefaults: usize,
    pub varargs: bool,
    pub nslots: usize,
    pub code: Vec<Op>,
    pub lines: Vec<u32>,
    pub consts: Vec<ValueSnap>,
    /// Indices into [`VmSnapshot::protos`] (always lower than this one's).
    pub protos: Vec<u32>,
    pub outers: Vec<OuterSource>,
    pub generator: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ObjectSnap {
    Table {
        nodes: Vec<(ValueSnap, ValueSnap, Option<u32>)>,
        first_free: u32,
        used: u32,
        delegate: Option<ObjRef>,
    },
    Array(Vec<ValueSnap>),
    Closure {
        proto: u32,
        outers: Vec<ValueSnap>,
        defaults: Vec<ValueSnap>,
        env: Option<ValueSnap>,
    },
    /// `native` indexes [`VmSnapshot::natives`].
    Native {
        native: u32,
        name: u32,
        nparams: i32,
        data: ValueSnap,
        env: Option<ValueSnap>,
    },
    Class {
        base: Option<ObjRef>,
        members: Vec<(ValueSnap, MemberKind, usize)>,
        fields: Vec<(ValueSnap, ValueSnap)>,
        methods: Vec<(ValueSnap, ValueSnap)>,
        metamethods: Vec<(Bytes, ValueSnap)>,
        attrs: ValueSnap,
        locked: bool,
        type_tag: u32,
    },
    Instance {
        class: ObjRef,
        fields: Vec<ValueSnap>,
        user: u64,
    },
    Generator {
        state: GenState,
        closure: ObjRef,
        stack: Vec<ValueSnap>,
        ip: usize,
        varargs: Vec<ValueSnap>,
        this: ValueSnap,
    },
    WeakRef(ValueSnap),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SlotSnap {
    pub generation: u32,
    pub obj: Option<ObjectSnap>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VmSnapshot {
    pub strings: Vec<Bytes>,
    pub protos: Vec<ProtoSnap>,
    /// Native function keys (`name#n`).
    pub natives: Vec<String>,
    pub slots: Vec<SlotSnap>,
    pub free: Vec<u32>,
    pub root: ObjRef,
    pub registry: ObjRef,
    /// Table, array, string, number, closure, generator, class, instance
    /// and weakref delegates.
    pub delegates: [ObjRef; 9],
    pub consts: Vec<(Bytes, ValueSnap)>,
    pub error_handler: ValueSnap,
    pub rand_state: u32,
}

struct Writer<'a> {
    vm: &'a Vm,
    strings: Vec<Bytes>,
    string_ids: HashMap<Vec<u8>, u32>,
    protos: Vec<ProtoSnap>,
    proto_ids: HashMap<*const Proto, u32>,
    natives: Vec<String>,
    native_ids: HashMap<String, u32>,
}

impl Writer<'_> {
    fn string(&mut self, s: &SqStr) -> u32 {
        if let Some(&i) = self.string_ids.get(s.bytes()) {
            return i;
        }
        let i = self.strings.len() as u32;
        self.strings.push(Bytes::of(s.bytes()));
        self.string_ids.insert(s.bytes().to_vec(), i);
        i
    }

    fn value(&mut self, v: &Value) -> ValueSnap {
        match v {
            Value::Null => ValueSnap::Null,
            Value::Bool(b) => ValueSnap::Bool(*b),
            Value::Int(i) => ValueSnap::Int(*i),
            Value::Float(f) => ValueSnap::Float(*f),
            Value::Str(s) => ValueSnap::Str(self.string(s)),
            Value::Table(r) => ValueSnap::Table(*r),
            Value::Array(r) => ValueSnap::Array(*r),
            Value::Closure(r) => ValueSnap::Closure(*r),
            Value::Native(r) => ValueSnap::Native(*r),
            Value::Class(r) => ValueSnap::Class(*r),
            Value::Instance(r) => ValueSnap::Instance(*r),
            Value::Generator(r) => ValueSnap::Generator(*r),
            Value::WeakRef(r) => ValueSnap::WeakRef(*r),
        }
    }

    fn values(&mut self, vs: &[Value]) -> Vec<ValueSnap> {
        vs.iter().map(|v| self.value(v)).collect()
    }

    fn proto(&mut self, p: &Arc<Proto>) -> u32 {
        if let Some(&i) = self.proto_ids.get(&Arc::as_ptr(p)) {
            return i;
        }
        let protos = p.protos.iter().map(|c| self.proto(c)).collect();
        let snap = ProtoSnap {
            name: self.string(&p.name),
            source: self.string(&p.source),
            params: p.params.iter().map(|s| self.string(s)).collect(),
            ndefaults: p.ndefaults,
            varargs: p.varargs,
            nslots: p.nslots,
            code: p.code.clone(),
            lines: p.lines.clone(),
            consts: self.values(&p.consts),
            protos,
            outers: p.outers.clone(),
            generator: p.generator,
        };
        let i = self.protos.len() as u32;
        self.protos.push(snap);
        self.proto_ids.insert(Arc::as_ptr(p), i);
        i
    }

    fn native(&mut self, n: &NativeClosure) -> Result<u32, String> {
        let entries = &self.vm.native_fns;
        let at = entries
            .iter()
            .position(|(name, f)| std::ptr::fn_addr_eq(*f, n.func) && name.bytes() == n.name.bytes())
            .ok_or_else(|| format!("native {} was not made by the VM", String::from_utf8_lossy(n.name.bytes())))?;
        let ordinal = entries[..at].iter().filter(|(name, _)| name.bytes() == n.name.bytes()).count();
        let key = format!("{}#{ordinal}", String::from_utf8_lossy(n.name.bytes()));
        if let Some(&i) = self.native_ids.get(&key) {
            return Ok(i);
        }
        let i = self.natives.len() as u32;
        self.natives.push(key.clone());
        self.native_ids.insert(key, i);
        Ok(i)
    }

    fn object(&mut self, obj: &Object, map_user: &mut dyn FnMut(u32, u64) -> u64) -> Result<ObjectSnap, String> {
        Ok(match obj {
            Object::Table(t) => ObjectSnap::Table {
                nodes: t.map.nodes.iter().map(|n| (self.value(&n.key), self.value(&n.val), n.next.map(|x| x as u32))).collect(),
                first_free: t.map.first_free as u32,
                used: t.map.used as u32,
                delegate: t.delegate,
            },
            Object::Array(a) => ObjectSnap::Array(self.values(a)),
            Object::Closure(c) => ObjectSnap::Closure {
                proto: self.proto(&c.proto),
                outers: self.values(&c.outers),
                defaults: self.values(&c.defaults),
                env: c.env.as_ref().map(|v| self.value(v)),
            },
            Object::Native(n) => ObjectSnap::Native {
                native: self.native(n)?,
                name: self.string(&n.name),
                nparams: n.nparams,
                data: self.value(&n.data),
                env: n.env.as_ref().map(|v| self.value(v)),
            },
            Object::Class(c) => ObjectSnap::Class {
                base: c.base,
                members: c.members.iter().map(|(k, kind, i)| (self.value(k), *kind, *i)).collect(),
                fields: c.fields.iter().map(|(a, b)| (self.value(a), self.value(b))).collect(),
                methods: c.methods.iter().map(|(a, b)| (self.value(a), self.value(b))).collect(),
                metamethods: c.metamethods.iter().map(|(n, v)| (Bytes::of(n), self.value(v))).collect(),
                attrs: self.value(&c.attrs),
                locked: c.locked,
                type_tag: c.type_tag,
            },
            Object::Instance(i) => {
                let tag = self.vm.heap.class(i.class).type_tag;
                ObjectSnap::Instance { class: i.class, fields: self.values(&i.fields), user: map_user(tag, i.user) }
            }
            Object::Generator(g) => ObjectSnap::Generator {
                state: g.state,
                closure: g.closure,
                stack: self.values(&g.stack),
                ip: g.ip,
                varargs: self.values(&g.varargs),
                this: self.value(&g.this),
            },
            Object::WeakRef(v) => ObjectSnap::WeakRef(self.value(v)),
        })
    }
}

struct Reader<'a> {
    strings: Vec<SqStr>,
    protos: Vec<Arc<Proto>>,
    natives: Vec<NativeFn>,
    snap: &'a VmSnapshot,
}

impl Reader<'_> {
    fn string(&self, i: u32) -> Result<SqStr, String> {
        self.strings.get(i as usize).cloned().ok_or_else(|| format!("bad string {i}"))
    }

    fn value(&self, v: &ValueSnap) -> Result<Value, String> {
        Ok(match v {
            ValueSnap::Null => Value::Null,
            ValueSnap::Bool(b) => Value::Bool(*b),
            ValueSnap::Int(i) => Value::Int(*i),
            ValueSnap::Float(f) => Value::Float(*f),
            ValueSnap::Str(i) => Value::Str(self.string(*i)?),
            ValueSnap::Table(r) => Value::Table(*r),
            ValueSnap::Array(r) => Value::Array(*r),
            ValueSnap::Closure(r) => Value::Closure(*r),
            ValueSnap::Native(r) => Value::Native(*r),
            ValueSnap::Class(r) => Value::Class(*r),
            ValueSnap::Instance(r) => Value::Instance(*r),
            ValueSnap::Generator(r) => Value::Generator(*r),
            ValueSnap::WeakRef(r) => Value::WeakRef(*r),
        })
    }

    fn values(&self, vs: &[ValueSnap]) -> Result<Vec<Value>, String> {
        vs.iter().map(|v| self.value(v)).collect()
    }

    fn proto(&self, i: u32) -> Result<Arc<Proto>, String> {
        self.protos.get(i as usize).cloned().ok_or_else(|| format!("bad function {i}"))
    }

    fn object(&self, o: &ObjectSnap) -> Result<Object, String> {
        Ok(match o {
            ObjectSnap::Table { nodes, first_free, used, delegate } => {
                let nodes = nodes
                    .iter()
                    .map(|(k, v, next)| Ok(Node { key: self.value(k)?, val: self.value(v)?, next: next.map(|x| x as usize) }))
                    .collect::<Result<Vec<_>, String>>()?;
                let map = SqTable { nodes, first_free: *first_free as usize, used: *used as usize };
                Object::Table(Table { map, delegate: *delegate })
            }
            ObjectSnap::Array(a) => Object::Array(self.values(a)?),
            ObjectSnap::Closure { proto, outers, defaults, env } => Object::Closure(Closure {
                proto: self.proto(*proto)?,
                outers: self.values(outers)?,
                defaults: self.values(defaults)?,
                env: env.as_ref().map(|v| self.value(v)).transpose()?,
            }),
            ObjectSnap::Native { native, name, nparams, data, env } => Object::Native(NativeClosure {
                name: self.string(*name)?,
                func: *self.natives.get(*native as usize).ok_or("bad native")?,
                nparams: *nparams,
                data: self.value(data)?,
                env: env.as_ref().map(|v| self.value(v)).transpose()?,
            }),
            ObjectSnap::Class { base, members, fields, methods, metamethods, attrs, locked, type_tag } => Object::Class(Class {
                base: *base,
                members: members.iter().map(|(k, kind, i)| Ok((self.value(k)?, *kind, *i))).collect::<Result<_, String>>()?,
                fields: fields.iter().map(|(a, b)| Ok((self.value(a)?, self.value(b)?))).collect::<Result<_, String>>()?,
                methods: methods.iter().map(|(a, b)| Ok((self.value(a)?, self.value(b)?))).collect::<Result<_, String>>()?,
                metamethods: metamethods.iter().map(|(n, v)| Ok((n.bytes().to_vec(), self.value(v)?))).collect::<Result<_, String>>()?,
                attrs: self.value(attrs)?,
                locked: *locked,
                type_tag: *type_tag,
            }),
            ObjectSnap::Instance { class, fields, user } => {
                Object::Instance(Instance { class: *class, fields: self.values(fields)?, user: *user })
            }
            ObjectSnap::Generator { state, closure, stack, ip, varargs, this } => Object::Generator(Generator {
                state: *state,
                closure: *closure,
                stack: self.values(stack)?,
                ip: *ip,
                varargs: self.values(varargs)?,
                this: self.value(this)?,
            }),
            ObjectSnap::WeakRef(v) => Object::WeakRef(self.value(v)?),
        })
    }
}

/// A value held by the host outside the heap (an entity's scope, a think
/// function), in saved form; references stay valid against a restored heap.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum HostValue {
    Null,
    Bool(bool),
    Int(i32),
    Float(f32),
    Str(Bytes),
    Table(ObjRef),
    Array(ObjRef),
    Closure(ObjRef),
    Native(ObjRef),
    Class(ObjRef),
    Instance(ObjRef),
    Generator(ObjRef),
    WeakRef(ObjRef),
}

impl HostValue {
    pub fn of(v: &Value) -> Self {
        match v {
            Value::Null => HostValue::Null,
            Value::Bool(b) => HostValue::Bool(*b),
            Value::Int(i) => HostValue::Int(*i),
            Value::Float(f) => HostValue::Float(*f),
            Value::Str(s) => HostValue::Str(Bytes::of(s.bytes())),
            Value::Table(r) => HostValue::Table(*r),
            Value::Array(r) => HostValue::Array(*r),
            Value::Closure(r) => HostValue::Closure(*r),
            Value::Native(r) => HostValue::Native(*r),
            Value::Class(r) => HostValue::Class(*r),
            Value::Instance(r) => HostValue::Instance(*r),
            Value::Generator(r) => HostValue::Generator(*r),
            Value::WeakRef(r) => HostValue::WeakRef(*r),
        }
    }

    pub fn value(&self) -> Value {
        match self {
            HostValue::Null => Value::Null,
            HostValue::Bool(b) => Value::Bool(*b),
            HostValue::Int(i) => Value::Int(*i),
            HostValue::Float(f) => Value::Float(*f),
            HostValue::Str(s) => Value::Str(SqStr::new(s.bytes())),
            HostValue::Table(r) => Value::Table(*r),
            HostValue::Array(r) => Value::Array(*r),
            HostValue::Closure(r) => Value::Closure(*r),
            HostValue::Native(r) => Value::Native(*r),
            HostValue::Class(r) => Value::Class(*r),
            HostValue::Instance(r) => Value::Instance(*r),
            HostValue::Generator(r) => Value::Generator(*r),
            HostValue::WeakRef(r) => Value::WeakRef(*r),
        }
    }
}

impl Vm {
    /// Live instances whose class has host type tag `tag`, with their host
    /// data.
    pub fn instances_with_tag(&self, tag: u32) -> Vec<(ObjRef, u64)> {
        self.heap
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| match &s.obj {
                Some(Object::Instance(inst)) if self.heap.class(inst.class).type_tag == tag => {
                    Some((ObjRef { index: i as u32, generation: s.generation }, inst.user))
                }
                _ => None,
            })
            .collect()
    }

    /// Snapshot of the whole VM. Only valid between calls (nothing
    /// running). `map_user` rewrites instance host data, given the type tag
    /// of the instance's class.
    pub fn snapshot(&self, map_user: &mut dyn FnMut(u32, u64) -> u64) -> Result<VmSnapshot, String> {
        if !self.frames.is_empty() || !self.stack.is_empty() {
            return Err("the VM is running".into());
        }
        let mut w = Writer {
            vm: self,
            strings: Vec::new(),
            string_ids: HashMap::new(),
            protos: Vec::new(),
            proto_ids: HashMap::new(),
            natives: Vec::new(),
            native_ids: HashMap::new(),
        };
        let mut slots = Vec::with_capacity(self.heap.slots.len());
        for s in &self.heap.slots {
            let obj = match &s.obj {
                Some(o) => Some(w.object(o, map_user)?),
                None => None,
            };
            slots.push(SlotSnap { generation: s.generation, obj });
        }
        let mut consts: Vec<(&Vec<u8>, &Value)> = self.consts.iter().collect();
        consts.sort_by(|a, b| a.0.cmp(b.0));
        let consts = consts.into_iter().map(|(k, v)| (Bytes::of(k), w.value(v))).collect();
        let error_handler = w.value(&self.error_handler);
        let d = &self.delegates;
        Ok(VmSnapshot {
            delegates: [d.table, d.array, d.string, d.number, d.closure, d.generator, d.class, d.instance, d.weakref],
            strings: w.strings,
            protos: w.protos,
            natives: w.natives,
            slots,
            free: self.heap.free.clone(),
            root: self.root,
            registry: self.registry,
            consts,
            error_handler,
            rand_state: self.rand_state,
        })
    }

    /// Replaces this VM's state with a snapshot. The VM must have been set
    /// up (natives registered) the same way as the one that was saved, so
    /// every saved native can be found again; `map_user` rewrites instance
    /// host data (type tag, saved data) back.
    pub fn restore(&mut self, snap: &VmSnapshot, map_user: &mut dyn FnMut(u32, u64) -> u64) -> Result<(), String> {
        let mut keys: HashMap<String, NativeFn> = HashMap::new();
        let mut seen: HashMap<Vec<u8>, usize> = HashMap::new();
        for (name, f) in &self.native_fns {
            let n = seen.entry(name.bytes().to_vec()).or_default();
            keys.insert(format!("{}#{n}", String::from_utf8_lossy(name.bytes())), *f);
            *n += 1;
        }
        let natives = snap
            .natives
            .iter()
            .map(|k| keys.get(k).copied().ok_or_else(|| format!("native {k} is not available")))
            .collect::<Result<Vec<_>, String>>()?;
        let strings: Vec<SqStr> = snap.strings.iter().map(|s| SqStr::new(s.bytes())).collect();
        let mut r = Reader { strings, protos: Vec::with_capacity(snap.protos.len()), natives, snap };
        for p in &snap.protos {
            let proto = Proto {
                name: r.string(p.name)?,
                source: r.string(p.source)?,
                params: p.params.iter().map(|&s| r.string(s)).collect::<Result<_, String>>()?,
                ndefaults: p.ndefaults,
                varargs: p.varargs,
                nslots: p.nslots,
                code: p.code.clone(),
                lines: p.lines.clone(),
                consts: r.values(&p.consts)?,
                protos: p.protos.iter().map(|&c| r.proto(c)).collect::<Result<_, String>>()?,
                outers: p.outers.clone(),
                generator: p.generator,
            };
            r.protos.push(Arc::new(proto));
        }
        let mut slots = Vec::with_capacity(r.snap.slots.len());
        for s in &r.snap.slots {
            let obj = match &s.obj {
                Some(o) => Some(r.object(o)?),
                None => None,
            };
            slots.push(Slot { generation: s.generation, marked: false, obj });
        }
        let consts = snap.consts.iter().map(|(k, v)| Ok((k.bytes().to_vec(), r.value(v)?))).collect::<Result<_, String>>()?;
        let error_handler = r.value(&snap.error_handler)?;
        self.heap.slots = slots;
        self.heap.free = snap.free.clone();
        self.heap.allocated_since_collect = 0;
        // Host data last: the class tags are on the heap now.
        let tags: Vec<(usize, u32)> = self
            .heap
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| match &s.obj {
                Some(Object::Instance(inst)) => Some((i, self.heap.class(inst.class).type_tag)),
                _ => None,
            })
            .collect();
        for (i, tag) in tags {
            if let Some(Object::Instance(inst)) = &mut self.heap.slots[i].obj {
                inst.user = map_user(tag, inst.user);
            }
        }
        let [table, array, string, number, closure, generator, class, instance, weakref] = snap.delegates;
        self.delegates = Delegates { table, array, string, number, closure, generator, class, instance, weakref };
        self.root = snap.root;
        self.registry = snap.registry;
        self.consts = consts;
        self.error_handler = error_handler;
        self.rand_state = snap.rand_state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(host: &mut dyn std::any::Any, text: &str) {
        if let Some(out) = host.downcast_mut::<String>() {
            out.push_str(text);
        }
    }

    fn run(vm: &mut Vm, src: &str) -> String {
        let mut out = String::new();
        let root = vm.root_value();
        if let Err(e) = vm.run_script(&mut out, src.as_bytes(), "test", &root) {
            out.push_str(&format!("<error: {}>", e.message()));
        }
        out
    }

    const SETUP: &str = r#"
        enum Colour { Red, Green = 5 }
        class Counter {
            n = 0
            step = 1
            constructor(s) { step = s }
            function bump() { n += step; return n }
            function _tostring() { return "Counter(" + n + ")" }
        }
        ::c <- Counter(3)
        ::t <- {}
        for (local i = 0; i < 40; i++) t["k" + i] <- i * i
        t[c] <- "object key"
        local base = 10
        ::adder <- function(x) { return x + base }
        ::gen <- (function() { local i = 0; while (true) { yield i; i += 2 } })()
        resume gen; resume gen
        ::weak <- c.weakref()
        ::re <- regexp("^On.*Output$")
        ::arr <- [1, 2.5, "three", null, true]
        ::colour <- Colour.Green
    "#;

    const PROBE: &str = r#"
        local keys = ""
        foreach (k, v in t) keys += (typeof k == "string" ? k : "obj") + ","
        print(keys + "|" + c.bump() + "|" + c + "|" + adder(5) + "|" + resume gen + "|" + resume gen)
        print("|" + (weak.ref() == c) + "|" + re.match("OnFooOutput") + "|" + arr.len() + arr[2] + "|" + colour)
        ::t.extra <- 1
        local after = ""
        foreach (k, v in t) after += (typeof k == "string" ? k : "obj") + ","
        print("|" + after)
    "#;

    #[test]
    fn restores_running_state() {
        let mut a = Vm::new();
        a.print = capture;
        assert_eq!(run(&mut a, SETUP), "");
        let snap = a.snapshot(&mut |_, u| u).unwrap();
        // Through the save file encoding as well.
        let mut bytes = Vec::new();
        ciborium::into_writer(&snap, &mut bytes).unwrap();
        let snap: VmSnapshot = ciborium::from_reader(bytes.as_slice()).unwrap();
        let expected = run(&mut a, PROBE);
        assert!(!expected.contains("<error"), "{expected}");

        let mut b = Vm::new();
        b.print = capture;
        b.restore(&snap, &mut |_, u| u).unwrap();
        assert_eq!(run(&mut b, PROBE), expected);
        b.collect_garbage();
        assert_eq!(run(&mut b, "print(c.bump())"), "6");
    }

    #[test]
    fn instance_data_is_mapped() {
        let mut a = Vm::new();
        let class = a.new_class(None, 77);
        let Value::Class(cr) = class else { unreachable!() };
        let inst = a.new_instance(cr, 5);
        let root = a.root_value();
        a.raw_set(&root, Value::str("h"), inst);
        let snap = a.snapshot(&mut |tag, u| if tag == 77 { u + 100 } else { u }).unwrap();
        let mut b = Vm::new();
        b.restore(&snap, &mut |tag, u| if tag == 77 { u * 2 } else { u }).unwrap();
        let Some(Value::Instance(r)) = b.raw_get(&b.root_value(), &Value::str("h")) else { panic!() };
        assert_eq!(b.heap.instance(r).user, 210);
    }
}
