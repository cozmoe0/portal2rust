//! The interpreter: call frames, the bytecode loop, and the object
//! semantics of Squirrel 2.2 (slot lookup through delegates with a root
//! table fallback for `this`, metamethods, arithmetic and comparison rules,
//! classes, generators and exceptions).

use std::any::Any;
use std::sync::Arc;

use crate::ast::{BinOp, UnOp};
use crate::compiler::{ConstBuilder, ConstTable, Op, OuterSource, Proto};
use crate::error::{Error, Result};
use crate::format::format_g;
use crate::table::SqTable;
use crate::value::*;

/// What a native function sees: the VM and the host it was called for.
pub struct Ctx<'a> {
    pub vm: &'a mut Vm,
    pub host: &'a mut dyn Any,
}

impl Ctx<'_> {
    /// Calls a script value from native code.
    pub fn call(&mut self, func: &Value, this: &Value, args: &[Value]) -> Result<Value> {
        self.vm.call(self.host, func, this, args)
    }

    pub fn host<T: 'static>(&mut self) -> Option<&mut T> {
        self.host.downcast_mut::<T>()
    }

    pub fn to_string(&mut self, v: &Value) -> Result<SqStr> {
        self.vm.to_string(self.host, v)
    }
}

struct Frame {
    closure: ObjRef,
    proto: Arc<Proto>,
    /// Stack index of slot 0 (`this`).
    base: usize,
    /// Stack height to restore when the frame ends (below the callee).
    floor: usize,
    ip: usize,
    varargs: Vec<Value>,
    generator: Option<ObjRef>,
    traps_base: usize,
}

struct Trap {
    frame: usize,
    ip: usize,
    stack_len: usize,
}

/// Default delegates: the methods every value of a type has.
#[derive(Clone, Copy)]
pub struct Delegates {
    pub table: ObjRef,
    pub array: ObjRef,
    pub string: ObjRef,
    pub number: ObjRef,
    pub closure: ObjRef,
    pub generator: ObjRef,
    pub class: ObjRef,
    pub instance: ObjRef,
    pub weakref: ObjRef,
}

/// Prints text from `print` and friends.
pub type PrintFn = fn(&mut dyn Any, &str);

fn default_print(_host: &mut dyn Any, text: &str) {
    print!("{text}");
}

pub struct Vm {
    pub heap: Heap,
    pub root: ObjRef,
    /// Host-owned values kept alive across collections.
    pub registry: ObjRef,
    pub consts: ConstTable,
    pub delegates: Delegates,
    pub print: PrintFn,
    pub error_handler: Value,
    pub rand_state: u32,
    stack: Vec<Value>,
    frames: Vec<Frame>,
    traps: Vec<Trap>,
    /// Depth of nested native/host calls, to bound recursion.
    depth: usize,
}

const MAX_DEPTH: usize = 200;

struct HeapBuilder<'a>(&'a mut Heap);

impl ConstBuilder for HeapBuilder<'_> {
    fn make_table(&mut self, entries: Vec<(Value, Value)>) -> Value {
        let mut map = SqTable::with_capacity(entries.len());
        for (k, v) in entries {
            map.new_slot(k, v);
        }
        Value::Table(self.0.alloc(Object::Table(Table { map, delegate: None })))
    }
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    pub fn new() -> Self {
        let mut heap = Heap::default();
        let table = |heap: &mut Heap| heap.alloc(Object::Table(Table { map: SqTable::default(), delegate: None }));
        let root = table(&mut heap);
        let registry = table(&mut heap);
        let delegates = Delegates {
            table: table(&mut heap),
            array: table(&mut heap),
            string: table(&mut heap),
            number: table(&mut heap),
            closure: table(&mut heap),
            generator: table(&mut heap),
            class: table(&mut heap),
            instance: table(&mut heap),
            weakref: table(&mut heap),
        };
        let mut vm = Self {
            heap,
            root,
            registry,
            consts: ConstTable::new(),
            delegates,
            print: default_print,
            error_handler: Value::Null,
            rand_state: 1,
            stack: Vec::new(),
            frames: Vec::new(),
            traps: Vec::new(),
            depth: 0,
        };
        crate::stdlib::register(&mut vm);
        vm
    }

    // ------------------------------------------------------------ host API

    pub fn root_value(&self) -> Value {
        Value::Table(self.root)
    }

    pub fn new_table(&mut self) -> Value {
        Value::Table(self.heap.alloc(Object::Table(Table { map: SqTable::default(), delegate: None })))
    }

    pub fn new_array(&mut self, items: Vec<Value>) -> Value {
        Value::Array(self.heap.alloc(Object::Array(items)))
    }

    pub fn new_native(&mut self, name: &str, func: NativeFn, nparams: i32, data: Value) -> Value {
        Value::Native(self.heap.alloc(Object::Native(NativeClosure { name: SqStr::from(name), func, nparams, data, env: None })))
    }

    pub fn new_class(&mut self, base: Option<ObjRef>, type_tag: u32) -> Value {
        let class = match base {
            Some(b) => {
                let mut c = self.heap.class(b).clone();
                c.base = Some(b);
                c.locked = false;
                c.attrs = Value::Null;
                c.type_tag = type_tag.max(c.type_tag);
                c
            }
            None => Class {
                base: None,
                members: Vec::new(),
                fields: Vec::new(),
                methods: Vec::new(),
                metamethods: Vec::new(),
                attrs: Value::Null,
                locked: false,
                type_tag,
            },
        };
        Value::Class(self.heap.alloc(Object::Class(class)))
    }

    /// Instance of a class without running its constructor (host handles).
    pub fn new_instance(&mut self, class: ObjRef, user: u64) -> Value {
        let c = self.heap.class_mut(class);
        c.locked = true;
        let fields = c.fields.iter().map(|(v, _)| v.clone()).collect();
        Value::Instance(self.heap.alloc(Object::Instance(Instance { class, fields, user })))
    }

    /// Sets (creating if needed) a slot of a table, bypassing metamethods.
    pub fn raw_set(&mut self, table: &Value, key: Value, val: Value) {
        if let Value::Table(t) = table {
            self.heap.table_mut(*t).map.new_slot(key, val);
        }
    }

    pub fn raw_get(&self, table: &Value, key: &Value) -> Option<Value> {
        match table {
            Value::Table(t) => self.heap.table(*t).map.get(key).cloned(),
            _ => None,
        }
    }

    /// Registers a native function in a table under `name`.
    pub fn set_native(&mut self, table: &Value, name: &str, func: NativeFn, nparams: i32) {
        let f = self.new_native(name, func, nparams, Value::Null);
        self.raw_set(table, Value::str(name), f);
    }

    pub fn set_delegate(&mut self, table: &Value, delegate: Option<ObjRef>) {
        if let Value::Table(t) = table {
            self.heap.table_mut(*t).delegate = delegate;
        }
    }

    /// Adds a method to a class (fails once the class has instances).
    pub fn class_new_slot(&mut self, class: ObjRef, key: Value, val: Value, is_static: bool, attrs: Value) -> Result<()> {
        let c = self.heap.class_mut(class);
        if c.locked {
            return Err(Error::runtime("trying to modify a class that has already been instantiated"));
        }
        if let Some((MemberKind::Field, i)) = c.find(&key) {
            c.fields[i] = (val, attrs);
            return Ok(());
        }
        let is_fn = matches!(val, Value::Closure(_) | Value::Native(_));
        if is_fn || is_static {
            if is_fn
                && let Value::Str(name) = &key
                && METAMETHODS.contains(&name.bytes())
            {
                let name = name.bytes().to_vec();
                match c.metamethods.iter_mut().find(|(n, _)| *n == name) {
                    Some(m) => m.1 = val,
                    None => c.metamethods.push((name, val)),
                }
                return Ok(());
            }
            match c.find(&key) {
                Some((MemberKind::Method, i)) => c.methods[i] = (val, attrs),
                _ => {
                    c.members.push((key, MemberKind::Method, c.methods.len()));
                    c.methods.push((val, attrs));
                }
            }
            return Ok(());
        }
        c.members.push((key, MemberKind::Field, c.fields.len()));
        c.fields.push((val, attrs));
        Ok(())
    }

    /// Compiles a script into a callable closure.
    pub fn compile(&mut self, src: &[u8], name: &str) -> Result<Value> {
        let stmts = crate::parser::parse(src, name)?;
        let mut builder = HeapBuilder(&mut self.heap);
        let proto = crate::compiler::compile(&stmts, name, &mut self.consts, &mut builder)?;
        Ok(Value::Closure(self.heap.alloc(Object::Closure(Closure { proto, outers: Vec::new(), defaults: Vec::new(), env: None }))))
    }

    /// Compiles and runs a script with `this` as its environment.
    pub fn run_script(&mut self, host: &mut dyn Any, src: &[u8], name: &str, this: &Value) -> Result<Value> {
        let f = self.compile(src, name)?;
        self.call(host, &f, this, &[])
    }

    /// Calls any callable value.
    pub fn call(&mut self, host: &mut dyn Any, func: &Value, this: &Value, args: &[Value]) -> Result<Value> {
        if self.depth >= MAX_DEPTH {
            return Err(Error::runtime("stack overflow"));
        }
        self.depth += 1;
        let sp = self.stack.len();
        self.stack.push(func.clone());
        self.stack.push(this.clone());
        self.stack.extend(args.iter().cloned());
        let r = self.call_at(host, sp, args.len(), true);
        self.stack.truncate(sp);
        self.depth -= 1;
        r.map(|v| v.unwrap_or_default())
    }

    /// Frees unreachable objects. Only call while no script is running.
    pub fn collect_garbage(&mut self) {
        self.collect_garbage_with(&[]);
    }

    /// Like `collect_garbage`, also keeping `extra` (host-held values) alive.
    pub fn collect_garbage_with(&mut self, extra: &[Value]) {
        debug_assert!(self.frames.is_empty());
        let mut roots = vec![Value::Table(self.root), Value::Table(self.registry), self.error_handler.clone()];
        let d = self.delegates;
        roots.extend(
            [d.table, d.array, d.string, d.number, d.closure, d.generator, d.class, d.instance, d.weakref]
                .into_iter()
                .map(Value::Table),
        );
        roots.extend(self.consts.values().cloned());
        roots.extend(self.stack.iter().cloned());
        roots.extend(extra.iter().cloned());
        self.heap.collect(roots.iter());
    }

    // ----------------------------------------------------------- semantics

    pub fn type_of(&mut self, host: &mut dyn Any, v: &Value) -> Result<SqStr> {
        if let Some(m) = self.metamethod(v, b"_typeof") {
            let r = self.call(host, &m, v, &[])?;
            if let Value::Str(s) = r {
                return Ok(s);
            }
        }
        Ok(SqStr::from(v.type_name()))
    }

    pub fn to_string(&mut self, host: &mut dyn Any, v: &Value) -> Result<SqStr> {
        Ok(match v {
            Value::Str(s) => s.clone(),
            Value::Null => SqStr::from("null"),
            Value::Bool(b) => SqStr::from(if *b { "true" } else { "false" }),
            Value::Int(i) => SqStr::from(i.to_string().as_str()),
            Value::Float(f) => SqStr::from(format_g(*f as f64, 6).as_str()),
            other => {
                if let Some(m) = self.metamethod(other, b"_tostring") {
                    let r = self.call(host, &m, other, &[])?;
                    if let Value::Str(s) = r {
                        return Ok(s);
                    }
                }
                let addr = other.obj().map(|o| o.index).unwrap_or(0);
                SqStr::from(format!("({} : 0x{:08X})", other.type_name(), 0x1000_0000u32.wrapping_add(addr.wrapping_mul(16))).as_str())
            }
        })
    }

    /// A metamethod of a table's delegate or an instance's class.
    fn metamethod(&self, v: &Value, name: &[u8]) -> Option<Value> {
        match v {
            Value::Table(t) => {
                let d = self.heap.table(*t).delegate?;
                self.heap.table(d).map.get(&Value::Str(SqStr::new(name))).cloned()
            }
            Value::Instance(i) => {
                let class = self.heap.instance(*i).class;
                self.heap.class(class).metamethod(name).cloned()
            }
            _ => None,
        }
    }

    fn default_delegate(&self, v: &Value) -> Option<ObjRef> {
        let d = &self.delegates;
        Some(match v {
            Value::Table(_) => d.table,
            Value::Array(_) => d.array,
            Value::Str(_) => d.string,
            Value::Int(_) | Value::Float(_) | Value::Bool(_) => d.number,
            Value::Closure(_) | Value::Native(_) => d.closure,
            Value::Generator(_) => d.generator,
            Value::Class(_) => d.class,
            Value::Instance(_) => d.instance,
            Value::WeakRef(_) => d.weakref,
            Value::Null => return None,
        })
    }

    /// Slot lookup with delegation. `raw` skips metamethods and default
    /// delegates. Returns None when the slot does not exist.
    pub fn get(&mut self, host: &mut dyn Any, obj: &Value, key: &Value, raw: bool) -> Result<Option<Value>> {
        match obj {
            Value::Table(t) => {
                if let Some(v) = self.heap.table(*t).map.get(key) {
                    return Ok(Some(v.clone()));
                }
                if let Some(d) = self.heap.table(*t).delegate {
                    if let Some(v) = self.get(host, &Value::Table(d), key, raw)? {
                        return Ok(Some(v));
                    }
                    if !raw && let Some(m) = self.heap.table(d).map.get(&Value::str("_get")).cloned() {
                        return self.call_get_metamethod(host, &m, obj, key);
                    }
                }
                if raw {
                    return Ok(None);
                }
            }
            Value::Array(a) => {
                if let Some(i) = key.as_i32().filter(|_| matches!(key, Value::Int(_) | Value::Float(_))) {
                    let arr = self.heap.array(*a);
                    return Ok(if i >= 0 && (i as usize) < arr.len() { Some(arr[i as usize].clone()) } else { None });
                }
                if raw {
                    return Ok(None);
                }
            }
            Value::Instance(i) => {
                let inst = self.heap.instance(*i);
                let class = self.heap.class(inst.class);
                if let Some((kind, idx)) = class.find(key) {
                    return Ok(Some(match kind {
                        MemberKind::Field => inst.fields[idx].clone(),
                        MemberKind::Method => class.methods[idx].0.clone(),
                    }));
                }
                if raw {
                    return Ok(None);
                }
                if let Some(m) = class.metamethod(b"_get").cloned() {
                    return self.call_get_metamethod(host, &m, obj, key);
                }
            }
            Value::Class(c) => {
                let class = self.heap.class(*c);
                if let Some((kind, idx)) = class.find(key) {
                    return Ok(Some(match kind {
                        MemberKind::Field => class.fields[idx].0.clone(),
                        MemberKind::Method => class.methods[idx].0.clone(),
                    }));
                }
                if raw {
                    return Ok(None);
                }
            }
            Value::Str(s) => {
                if let Value::Int(n) = key {
                    let len = s.len() as i32;
                    let idx = if *n < 0 { len + n } else { *n };
                    return Ok((idx >= 0 && idx < len).then(|| Value::Int(s.bytes()[idx as usize] as i8 as i32)));
                }
                if raw {
                    return Ok(None);
                }
            }
            _ if raw => return Ok(None),
            _ => {}
        }
        match self.default_delegate(obj) {
            Some(d) => Ok(self.heap.table(d).map.get(key).cloned()),
            None => Ok(None),
        }
    }

    /// `_get` signals a missing slot by throwing; that reads as absent.
    fn call_get_metamethod(&mut self, host: &mut dyn Any, m: &Value, obj: &Value, key: &Value) -> Result<Option<Value>> {
        match self.call(host, m, obj, std::slice::from_ref(key)) {
            Ok(v) => Ok(Some(v)),
            Err(Error::Runtime { value: Value::Null, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Assignment to an existing slot. Returns false when it does not exist.
    pub fn set(&mut self, host: &mut dyn Any, obj: &Value, key: &Value, val: Value) -> Result<bool> {
        match obj {
            Value::Table(t) => {
                if self.heap.table_mut(*t).map.set(key, val.clone()) {
                    return Ok(true);
                }
                if let Some(d) = self.heap.table(*t).delegate {
                    if self.set(host, &Value::Table(d), key, val.clone())? {
                        return Ok(true);
                    }
                    if let Some(m) = self.heap.table(d).map.get(&Value::str("_set")).cloned() {
                        self.call(host, &m, obj, &[key.clone(), val])?;
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Value::Instance(i) => {
                let class = self.heap.instance(*i).class;
                if let Some((MemberKind::Field, idx)) = self.heap.class(class).find(key) {
                    self.heap.instance_mut(*i).fields[idx] = val;
                    return Ok(true);
                }
                if let Some(m) = self.heap.class(class).metamethod(b"_set").cloned() {
                    self.call(host, &m, obj, &[key.clone(), val])?;
                    return Ok(true);
                }
                Ok(false)
            }
            Value::Array(a) => {
                let Some(i) = key.as_i32().filter(|_| matches!(key, Value::Int(_) | Value::Float(_))) else {
                    return Err(Error::runtime(format!("indexing {} with {}", obj.type_name(), key.type_name())));
                };
                let arr = self.heap.array_mut(*a);
                if i >= 0 && (i as usize) < arr.len() {
                    arr[i as usize] = val;
                    return Ok(true);
                }
                Ok(false)
            }
            _ => Err(Error::runtime(format!("trying to set '{}'", obj.type_name()))),
        }
    }

    pub fn new_slot(&mut self, host: &mut dyn Any, obj: &Value, key: Value, val: Value, is_static: bool) -> Result<()> {
        if key.is_null() {
            return Err(Error::runtime("null cannot be used as index"));
        }
        match obj {
            Value::Table(t) => {
                let t = *t;
                if let Some(d) = self.heap.table(t).delegate
                    && !self.heap.table(t).map.contains(&key)
                    && let Some(m) = self.heap.table(d).map.get(&Value::str("_newslot")).cloned()
                {
                    self.call(host, &m, obj, &[key, val])?;
                    return Ok(());
                }
                self.heap.table_mut(t).map.new_slot(key, val);
                Ok(())
            }
            Value::Instance(i) => {
                let class = self.heap.instance(*i).class;
                match self.heap.class(class).metamethod(b"_newslot").cloned() {
                    Some(m) => {
                        self.call(host, &m, obj, &[key, val])?;
                        Ok(())
                    }
                    None => Err(Error::runtime("class instances do not support the new slot operator")),
                }
            }
            Value::Class(c) => {
                let c = *c;
                if self.heap.class(c).locked {
                    return Err(Error::runtime("trying to modify a class that has already been instantiated"));
                }
                self.class_new_slot(c, key, val, is_static, Value::Null)
            }
            _ => Err(Error::runtime(format!("indexing {} with {}", obj.type_name(), key.type_name()))),
        }
    }

    fn delete_slot(&mut self, host: &mut dyn Any, obj: &Value, key: &Value) -> Result<Value> {
        match obj {
            Value::Table(t) => {
                if let Some(m) = self.metamethod(obj, b"_delslot") {
                    return self.call(host, &m, obj, std::slice::from_ref(key));
                }
                match self.heap.table_mut(*t).map.remove(key) {
                    Some(v) => Ok(v),
                    None => Err(self.index_error(host, key)),
                }
            }
            Value::Instance(_) => match self.metamethod(obj, b"_delslot") {
                Some(m) => self.call(host, &m, obj, std::slice::from_ref(key)),
                None => Err(Error::runtime(format!("cannot delete a slot from {}", obj.type_name()))),
            },
            _ => Err(Error::runtime(format!("attempt to delete a slot from a {}", obj.type_name()))),
        }
    }

    fn index_error(&mut self, host: &mut dyn Any, key: &Value) -> Error {
        let shown = match key {
            Value::Str(s) => s.to_string_lossy(),
            other => self.to_string(host, other).map(|s| s.to_string_lossy()).unwrap_or_default(),
        };
        Error::runtime(format!("the index '{shown}' does not exist"))
    }

    /// `this`-relative lookup: the environment, then the root table.
    fn get_env(&mut self, host: &mut dyn Any, this: &Value, key: &Value) -> Result<Value> {
        if let Some(v) = self.get(host, this, key, false)? {
            return Ok(v);
        }
        if let Some(v) = self.heap.table(self.root).map.get(key) {
            return Ok(v.clone());
        }
        Err(self.index_error(host, key))
    }

    fn set_env(&mut self, host: &mut dyn Any, this: &Value, key: &Value, val: Value) -> Result<()> {
        if self.set(host, this, key, val.clone())? {
            return Ok(());
        }
        if self.heap.table_mut(self.root).map.set(key, val) {
            return Ok(());
        }
        Err(self.index_error(host, key))
    }

    pub fn equals(&self, a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits(),
            (Value::Int(x), Value::Float(y)) => (*x as f32) == *y,
            (Value::Float(x), Value::Int(y)) => *x == (*y as f32),
            _ => a.raw_eq(b),
        }
    }

    /// Three-way comparison following the original rules (null sorts first,
    /// same-typed objects compare by identity unless they have `_cmp`).
    pub fn compare(&mut self, host: &mut dyn Any, a: &Value, b: &Value) -> Result<i32> {
        use std::cmp::Ordering::*;
        let ord = |o: std::cmp::Ordering| match o {
            Less => -1,
            Equal => 0,
            Greater => 1,
        };
        match (a, b) {
            (Value::Int(x), Value::Int(y)) => Ok(ord(x.cmp(y))),
            (Value::Float(x), Value::Float(y)) => Ok(if x.to_bits() == y.to_bits() { 0 } else if x < y { -1 } else { 1 }),
            (Value::Int(x), Value::Float(y)) => {
                let x = *x as f32;
                Ok(if x == *y { 0 } else if x < *y { -1 } else { 1 })
            }
            (Value::Float(x), Value::Int(y)) => {
                let y = *y as f32;
                Ok(if *x == y { 0 } else if *x < y { -1 } else { 1 })
            }
            (Value::Str(x), Value::Str(y)) => Ok(ord(x.bytes().cmp(y.bytes()))),
            _ if std::mem::discriminant(a) == std::mem::discriminant(b) => {
                if a.raw_eq(b) {
                    return Ok(0);
                }
                if let Some(m) = self.metamethod(a, b"_cmp") {
                    return match self.call(host, &m, a, std::slice::from_ref(b))? {
                        Value::Int(i) => Ok(i),
                        _ => Err(Error::runtime(format!("comparison between '{}' and '{}'", a.type_name(), b.type_name()))),
                    };
                }
                let (x, y) = (a.obj().map(|o| o.index).unwrap_or(0), b.obj().map(|o| o.index).unwrap_or(0));
                Ok(if x < y { -1 } else { 1 })
            }
            (Value::Null, _) => Ok(-1),
            (_, Value::Null) => Ok(1),
            _ => Err(Error::runtime(format!("comparison between '{}' and '{}'", a.type_name(), b.type_name()))),
        }
    }

    fn arith(&mut self, host: &mut dyn Any, op: BinOp, a: &Value, b: &Value) -> Result<Value> {
        let sym = match op {
            BinOp::Add => '+',
            BinOp::Sub => '-',
            BinOp::Mul => '*',
            BinOp::Div => '/',
            _ => '%',
        };
        match (a, b) {
            (Value::Int(x), Value::Int(y)) => Ok(Value::Int(match op {
                BinOp::Add => x.wrapping_add(*y),
                BinOp::Sub => x.wrapping_sub(*y),
                BinOp::Mul => x.wrapping_mul(*y),
                BinOp::Div => {
                    if *y == 0 {
                        return Err(Error::runtime("division by zero"));
                    }
                    x.wrapping_div(*y)
                }
                _ => {
                    if *y == 0 {
                        return Err(Error::runtime("modulo by zero"));
                    }
                    x.wrapping_rem(*y)
                }
            })),
            _ if a.as_f32().is_some() && b.as_f32().is_some() && !matches!(a, Value::Bool(_)) && !matches!(b, Value::Bool(_)) => {
                let (x, y) = (a.as_f32().expect("numeric"), b.as_f32().expect("numeric"));
                Ok(Value::Float(match op {
                    BinOp::Add => x + y,
                    BinOp::Sub => x - y,
                    BinOp::Mul => x * y,
                    BinOp::Div => x / y,
                    _ => ((x as f64) % (y as f64)) as f32,
                }))
            }
            _ => {
                if op == BinOp::Add && (matches!(a, Value::Str(_)) || matches!(b, Value::Str(_))) {
                    let sa = self.to_string(host, a)?;
                    let sb = self.to_string(host, b)?;
                    let mut bytes = Vec::with_capacity(sa.len() + sb.len());
                    bytes.extend_from_slice(sa.bytes());
                    bytes.extend_from_slice(sb.bytes());
                    return Ok(Value::Str(SqStr::new(&bytes)));
                }
                let name: &[u8] = match op {
                    BinOp::Add => b"_add",
                    BinOp::Sub => b"_sub",
                    BinOp::Mul => b"_mul",
                    BinOp::Div => b"_div",
                    _ => b"_modulo",
                };
                if let Some(m) = self.metamethod(a, name) {
                    return self.call(host, &m, a, std::slice::from_ref(b));
                }
                Err(Error::runtime(format!("arith op {sym} on between '{}' and '{}'", a.type_name(), b.type_name())))
            }
        }
    }

    fn binary(&mut self, host: &mut dyn Any, op: BinOp, a: Value, b: Value) -> Result<Value> {
        Ok(match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => return self.arith(host, op, &a, &b),
            BinOp::Shl | BinOp::Shr | BinOp::UShr | BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                let (Value::Int(x), Value::Int(y)) = (&a, &b) else {
                    return Err(Error::runtime(format!("bitwise op between '{}' and '{}'", a.type_name(), b.type_name())));
                };
                Value::Int(match op {
                    BinOp::Shl => x.wrapping_shl(*y as u32),
                    BinOp::Shr => x.wrapping_shr(*y as u32),
                    BinOp::UShr => (*x as u32).wrapping_shr(*y as u32) as i32,
                    BinOp::BitAnd => x & y,
                    BinOp::BitOr => x | y,
                    _ => x ^ y,
                })
            }
            BinOp::Eq => Value::Bool(self.equals(&a, &b)),
            BinOp::Ne => Value::Bool(!self.equals(&a, &b)),
            BinOp::Lt => Value::Bool(self.compare(host, &a, &b)? < 0),
            BinOp::Le => Value::Bool(self.compare(host, &a, &b)? <= 0),
            BinOp::Gt => Value::Bool(self.compare(host, &a, &b)? > 0),
            BinOp::Ge => Value::Bool(self.compare(host, &a, &b)? >= 0),
            BinOp::In => Value::Bool(self.get(host, &b, &a, true)?.is_some()),
            BinOp::InstanceOf => {
                let (Value::Class(c), Value::Instance(i)) = (&b, &a) else {
                    return Err(Error::runtime(format!(
                        "cannot apply instanceof between a {} and a {}",
                        b.type_name(),
                        a.type_name()
                    )));
                };
                let mut cur = Some(self.heap.instance(*i).class);
                let mut found = false;
                while let Some(k) = cur {
                    if k == *c {
                        found = true;
                        break;
                    }
                    cur = self.heap.class(k).base;
                }
                Value::Bool(found)
            }
        })
    }

    fn unary(&mut self, host: &mut dyn Any, op: UnOp, v: Value) -> Result<Value> {
        Ok(match op {
            UnOp::Neg => match v {
                Value::Int(i) => Value::Int(i.wrapping_neg()),
                Value::Float(f) => Value::Float(-f),
                other => match self.metamethod(&other, b"_unm") {
                    Some(m) => self.call(host, &m, &other, &[])?,
                    None => return Err(Error::runtime(format!("attempt to negate a {}", other.type_name()))),
                },
            },
            UnOp::Not => Value::Bool(!v.truthy()),
            UnOp::BitNot => match v {
                Value::Int(i) => Value::Int(!i),
                other => return Err(Error::runtime(format!("attempt to perform a bitwise op on a {}", other.type_name()))),
            },
            UnOp::TypeOf => Value::Str(self.type_of(host, &v)?),
            UnOp::Clone => self.clone_value(host, &v)?,
            UnOp::Resume => match v {
                Value::Generator(g) => self.resume(host, g)?,
                other => {
                    return Err(Error::runtime(format!(
                        "trying to resume a '{}',only genenerator can be resumed",
                        other.type_name()
                    )));
                }
            },
        })
    }

    pub fn clone_value(&mut self, host: &mut dyn Any, v: &Value) -> Result<Value> {
        let copy = match v {
            Value::Table(t) => {
                let src = self.heap.table(*t);
                let table = Table { map: src.map.duplicate(), delegate: src.delegate };
                Value::Table(self.heap.alloc(Object::Table(table)))
            }
            Value::Instance(i) => {
                let inst = self.heap.instance(*i).clone();
                Value::Instance(self.heap.alloc(Object::Instance(inst)))
            }
            Value::Array(a) => {
                let items = self.heap.array(*a).clone();
                return Ok(self.new_array(items));
            }
            other => return Ok(other.clone()),
        };
        if let Some(m) = self.metamethod(&copy, b"_cloned") {
            self.call(host, &m, &copy, std::slice::from_ref(v))?;
        }
        Ok(copy)
    }

    /// Creates an instance and runs its constructor.
    fn instantiate(&mut self, host: &mut dyn Any, class: ObjRef, args: &[Value]) -> Result<Value> {
        let inst = self.new_instance(class, 0);
        let ctor = match self.heap.class(class).find(&Value::str("constructor")) {
            Some((MemberKind::Method, i)) => Some(self.heap.class(class).methods[i].0.clone()),
            _ => None,
        };
        if let Some(ctor) = ctor {
            self.call(host, &ctor, &inst, args)?;
        }
        Ok(inst)
    }

    // ---------------------------------------------------------------- calls

    /// Calls the value at `stack[sp]` with `this` at `sp + 1` and `argc`
    /// arguments after it. With `nested`, closures run to completion here;
    /// otherwise their frame is pushed for the running loop. Returns the
    /// result, or None when a frame was pushed.
    fn call_at(&mut self, host: &mut dyn Any, sp: usize, argc: usize, nested: bool) -> Result<Option<Value>> {
        let callee = self.stack[sp].clone();
        match callee {
            Value::Closure(c) => {
                let closure = self.heap.closure(c).clone();
                let proto = closure.proto.clone();
                let base = sp + 1;
                let nparams = proto.params.len();
                let mut nargs = argc;
                let mut varargs = Vec::new();
                if nargs != nparams {
                    let ndef = proto.ndefaults;
                    if nargs < nparams && ndef > 0 && nparams - nargs <= ndef {
                        let diff = nparams - nargs;
                        for n in ndef - diff..ndef {
                            self.stack.push(closure.defaults[n].clone());
                            nargs += 1;
                        }
                    } else if proto.varargs && nargs > nparams {
                        varargs = self.stack.split_off(base + 1 + nparams);
                        nargs = nparams;
                    } else if !(proto.varargs && nargs == nparams) {
                        return Err(Error::runtime("wrong number of parameters"));
                    }
                }
                let _ = nargs;
                if let Some(env) = &closure.env {
                    let this = match env {
                        Value::WeakRef(w) => self.heap.weak_target(*w),
                        other => other.clone(),
                    };
                    self.stack[base] = this;
                }
                self.stack.resize(base + proto.nslots.max(1 + nparams), Value::Null);
                if proto.generator {
                    let stack = self.stack.split_off(base);
                    let this = stack[0].clone();
                    let generator = Generator { state: GenState::Suspended, closure: c, stack, ip: 0, varargs, this };
                    self.stack.truncate(sp);
                    return Ok(Some(Value::Generator(self.heap.alloc(Object::Generator(generator)))));
                }
                self.frames.push(Frame {
                    closure: c,
                    proto,
                    base,
                    floor: sp,
                    ip: 0,
                    varargs,
                    generator: None,
                    traps_base: self.traps.len(),
                });
                if nested {
                    let stop = self.frames.len() - 1;
                    return self.execute(host, stop).map(Some);
                }
                Ok(None)
            }
            Value::Native(n) => {
                let native = self.heap.native(n).clone();
                let nargs = argc + 1;
                if (native.nparams > 0 && native.nparams as usize != nargs)
                    || (native.nparams < 0 && nargs < (-native.nparams) as usize)
                {
                    return Err(Error::runtime("wrong number of parameters"));
                }
                let mut args: Vec<Value> = self.stack[sp + 1..sp + 1 + nargs].to_vec();
                if let Some(env) = &native.env {
                    args[0] = env.clone();
                }
                let mut ctx = Ctx { vm: self, host };
                let r = (native.func)(&mut ctx, &args)?;
                Ok(Some(r))
            }
            Value::Class(c) => {
                let args: Vec<Value> = self.stack[sp + 2..sp + 2 + argc].to_vec();
                self.instantiate(host, c, &args).map(Some)
            }
            Value::Table(_) | Value::Instance(_) => match self.metamethod(&callee, b"_call") {
                Some(m) => {
                    let args: Vec<Value> = self.stack[sp + 1..sp + 2 + argc].to_vec();
                    self.call(host, &m, &callee, &args).map(Some)
                }
                None => Err(Error::runtime(format!("attempt to call '{}'", callee.type_name()))),
            },
            other => Err(Error::runtime(format!("attempt to call '{}'", other.type_name()))),
        }
    }

    fn resume(&mut self, host: &mut dyn Any, g: ObjRef) -> Result<Value> {
        let generator = self.heap.generator_mut(g);
        match generator.state {
            GenState::Dead => return Err(Error::runtime("resuming dead generator")),
            GenState::Running => return Err(Error::runtime("resuming active generator")),
            GenState::Suspended => {}
        }
        generator.state = GenState::Running;
        let saved = std::mem::take(&mut generator.stack);
        let varargs = std::mem::take(&mut generator.varargs);
        let (closure, ip) = (generator.closure, generator.ip);
        let proto = self.heap.closure(closure).proto.clone();
        if self.depth >= MAX_DEPTH {
            return Err(Error::runtime("stack overflow"));
        }
        self.depth += 1;
        let base = self.stack.len();
        self.stack.extend(saved);
        self.frames.push(Frame { closure, proto, base, floor: base, ip, varargs, generator: Some(g), traps_base: self.traps.len() });
        let stop = self.frames.len() - 1;
        let r = self.execute(host, stop);
        self.depth -= 1;
        if r.is_err() {
            self.heap.generator_mut(g).state = GenState::Dead;
        }
        r
    }

    // ------------------------------------------------------------- the loop

    /// Runs frames until the frame stack is back to `stop` frames.
    fn execute(&mut self, host: &mut dyn Any, stop: usize) -> Result<Value> {
        loop {
            match self.step(host, stop) {
                Ok(Some(v)) => return Ok(v),
                Ok(None) => {}
                Err(e) => {
                    let e = self.annotate(e);
                    // Innermost trap belonging to a frame of this run.
                    if let Some(t) = self.traps.last()
                        && t.frame >= stop
                        && t.frame < self.frames.len()
                    {
                        let t = self.traps.pop().expect("trap");
                        for f in self.frames.drain(t.frame + 1..) {
                            if let Some(g) = f.generator {
                                self.heap.generator_mut(g).state = GenState::Dead;
                            }
                        }
                        self.stack.truncate(t.stack_len);
                        let value = match &e {
                            Error::Runtime { value, .. } => value.clone(),
                            Error::Compile { msg, .. } => Value::str(msg),
                        };
                        self.stack.push(value);
                        self.frames[t.frame].ip = t.ip;
                        continue;
                    }
                    let floor = self.frames.get(stop).map(|f| f.floor);
                    for f in self.frames.drain(stop..) {
                        if let Some(g) = f.generator {
                            self.heap.generator_mut(g).state = GenState::Dead;
                        }
                    }
                    let traps_keep = self.traps.iter().position(|t| t.frame >= stop).unwrap_or(self.traps.len());
                    self.traps.truncate(traps_keep);
                    if let Some(f) = floor {
                        self.stack.truncate(f);
                    }
                    return Err(e);
                }
            }
        }
    }

    /// Adds the call stack to an error the first time it passes through.
    fn annotate(&self, e: Error) -> Error {
        match e {
            Error::Runtime { value, message, stack } if stack.is_empty() => {
                let stack = self
                    .frames
                    .iter()
                    .rev()
                    .map(|f| {
                        let line = f.proto.lines.get(f.ip.saturating_sub(1)).copied().unwrap_or(0);
                        format!("*FUNCTION [{}()] {} line [{}]", f.proto.name.to_string_lossy(), f.proto.source.to_string_lossy(), line)
                    })
                    .collect();
                Error::Runtime { value, message, stack }
            }
            other => other,
        }
    }

    fn pop(&mut self) -> Value {
        self.stack.pop().expect("stack underflow")
    }

    fn top(&self) -> &Value {
        self.stack.last().expect("stack underflow")
    }

    /// Executes one instruction. Returns a value when the run is complete.
    fn step(&mut self, host: &mut dyn Any, stop: usize) -> Result<Option<Value>> {
        let fi = self.frames.len() - 1;
        let (op, base) = {
            let f = &mut self.frames[fi];
            let op = f.proto.code[f.ip];
            f.ip += 1;
            (op, f.base)
        };
        match op {
            Op::Null => self.stack.push(Value::Null),
            Op::True => self.stack.push(Value::Bool(true)),
            Op::False => self.stack.push(Value::Bool(false)),
            Op::Int(i) => self.stack.push(Value::Int(i)),
            Op::Float(x) => self.stack.push(Value::Float(x)),
            Op::Const(k) => {
                let v = self.frames[fi].proto.consts[k as usize].clone();
                self.stack.push(v);
            }
            Op::This => {
                let v = self.stack[base].clone();
                self.stack.push(v);
            }
            Op::Root => self.stack.push(Value::Table(self.root)),
            Op::Parent => {
                let v = match &self.stack[base] {
                    Value::Table(t) => self.heap.table(*t).delegate.map(Value::Table).unwrap_or_default(),
                    Value::Class(c) => self.heap.class(*c).base.map(Value::Class).unwrap_or_default(),
                    other => return Err(Error::runtime(format!("the {} type doesn't have a parent slot", other.type_name()))),
                };
                self.stack.push(v);
            }
            Op::ParentOf => {
                let obj = self.pop();
                let v = match &obj {
                    Value::Table(t) => self.heap.table(*t).delegate.map(Value::Table).unwrap_or_default(),
                    Value::Class(c) => self.heap.class(*c).base.map(Value::Class).unwrap_or_default(),
                    other => return Err(Error::runtime(format!("the {} type doesn't have a parent slot", other.type_name()))),
                };
                self.stack.push(v);
            }
            Op::Base => {
                let v = match &self.stack[base] {
                    Value::Instance(i) => {
                        let class = self.heap.instance(*i).class;
                        self.heap.class(class).base.map(Value::Class).unwrap_or_default()
                    }
                    _ => Value::Null,
                };
                self.stack.push(v);
            }
            Op::Vargc => {
                let n = self.frames[fi].varargs.len() as i32;
                self.stack.push(Value::Int(n));
            }
            Op::Vargv => {
                let idx = self.pop();
                let f = &self.frames[fi];
                let v = match idx {
                    Value::Int(i) if i >= 0 && (i as usize) < f.varargs.len() => f.varargs[i as usize].clone(),
                    _ => return Err(Error::runtime("the index doesn't exist")),
                };
                self.stack.push(v);
            }
            Op::GetLocal(s) => {
                let v = self.stack[base + s as usize].clone();
                self.stack.push(v);
            }
            Op::SetLocal(s) => {
                let v = self.top().clone();
                self.stack[base + s as usize] = v;
            }
            Op::GetOuter(i) => {
                let c = self.frames[fi].closure;
                let v = self.heap.closure(c).outers[i as usize].clone();
                self.stack.push(v);
            }
            Op::SetOuter(i) => {
                let v = self.top().clone();
                let c = self.frames[fi].closure;
                if let Object::Closure(cl) = self.heap.get_mut(c) {
                    cl.outers[i as usize] = v;
                }
            }
            Op::GetEnv(k) => {
                let key = self.frames[fi].proto.consts[k as usize].clone();
                let this = self.stack[base].clone();
                let v = self.get_env(host, &this, &key)?;
                self.stack.push(v);
            }
            Op::SetEnv(k) => {
                let key = self.frames[fi].proto.consts[k as usize].clone();
                let this = self.stack[base].clone();
                let v = self.top().clone();
                self.set_env(host, &this, &key, v)?;
            }
            Op::NewSlotEnv(k) => {
                let key = self.frames[fi].proto.consts[k as usize].clone();
                let this = self.stack[base].clone();
                let v = self.top().clone();
                self.new_slot(host, &this, key, v, false)?;
            }
            Op::Get => {
                let key = self.pop();
                let obj = self.pop();
                match self.get(host, &obj, &key, false)? {
                    Some(v) => self.stack.push(v),
                    None => return Err(self.index_error(host, &key)),
                }
            }
            Op::GetK(k) => {
                let key = self.frames[fi].proto.consts[k as usize].clone();
                let obj = self.pop();
                match self.get(host, &obj, &key, false)? {
                    Some(v) => self.stack.push(v),
                    None => return Err(self.index_error(host, &key)),
                }
            }
            Op::Set => {
                let val = self.pop();
                let key = self.pop();
                let obj = self.pop();
                if !self.set(host, &obj, &key, val.clone())? {
                    return Err(self.index_error(host, &key));
                }
                self.stack.push(val);
            }
            Op::NewSlot => {
                let val = self.pop();
                let key = self.pop();
                let obj = self.pop();
                self.new_slot(host, &obj, key, val.clone(), false)?;
                self.stack.push(val);
            }
            Op::Delete => {
                let key = self.pop();
                let obj = self.pop();
                let v = self.delete_slot(host, &obj, &key)?;
                self.stack.push(v);
            }
            Op::DeleteEnv(k) => {
                let key = self.frames[fi].proto.consts[k as usize].clone();
                let this = self.stack[base].clone();
                let v = self.delete_slot(host, &this, &key)?;
                self.stack.push(v);
            }
            Op::Inc { delta, post } => {
                let key = self.pop();
                let obj = self.pop();
                let Some(old) = self.get(host, &obj, &key, false)? else { return Err(self.index_error(host, &key)) };
                let new = self.arith(host, BinOp::Add, &old, &Value::Int(delta as i32))?;
                if !self.set(host, &obj, &key, new.clone())? {
                    return Err(self.index_error(host, &key));
                }
                self.stack.push(if post { old } else { new });
            }
            Op::IncLocal { slot, delta, post } => {
                let old = self.stack[base + slot as usize].clone();
                let new = self.arith(host, BinOp::Add, &old, &Value::Int(delta as i32))?;
                self.stack[base + slot as usize] = new.clone();
                self.stack.push(if post { old } else { new });
            }
            Op::IncOuter { slot, delta, post } => {
                let c = self.frames[fi].closure;
                let old = self.heap.closure(c).outers[slot as usize].clone();
                let new = self.arith(host, BinOp::Add, &old, &Value::Int(delta as i32))?;
                if let Object::Closure(cl) = self.heap.get_mut(c) {
                    cl.outers[slot as usize] = new.clone();
                }
                self.stack.push(if post { old } else { new });
            }
            Op::IncEnv { name, delta, post } => {
                let key = self.frames[fi].proto.consts[name as usize].clone();
                let this = self.stack[base].clone();
                let old = self.get_env(host, &this, &key)?;
                let new = self.arith(host, BinOp::Add, &old, &Value::Int(delta as i32))?;
                self.set_env(host, &this, &key, new.clone())?;
                self.stack.push(if post { old } else { new });
            }
            Op::Pop => {
                self.pop();
            }
            Op::Dup => {
                let v = self.top().clone();
                self.stack.push(v);
            }
            Op::Dup2 => {
                let n = self.stack.len();
                let (a, b) = (self.stack[n - 2].clone(), self.stack[n - 1].clone());
                self.stack.push(a);
                self.stack.push(b);
            }
            Op::Swap => {
                let n = self.stack.len();
                self.stack.swap(n - 1, n - 2);
            }
            Op::Unary(u) => {
                let v = self.pop();
                let r = self.unary(host, u, v)?;
                self.stack.push(r);
            }
            Op::Binary(b) => {
                let rhs = self.pop();
                let lhs = self.pop();
                let r = self.binary(host, b, lhs, rhs)?;
                self.stack.push(r);
            }
            Op::Jump(t) => self.frames[fi].ip = t as usize,
            Op::JumpIfFalse(t) => {
                if !self.pop().truthy() {
                    self.frames[fi].ip = t as usize;
                }
            }
            Op::JumpIfTrue(t) => {
                if self.pop().truthy() {
                    self.frames[fi].ip = t as usize;
                }
            }
            Op::AndJump(t) => {
                if !self.top().truthy() {
                    self.frames[fi].ip = t as usize;
                } else {
                    self.pop();
                }
            }
            Op::OrJump(t) => {
                if self.top().truthy() {
                    self.frames[fi].ip = t as usize;
                } else {
                    self.pop();
                }
            }
            Op::Call(argc) => {
                let sp = self.stack.len() - argc as usize - 2;
                if self.depth >= MAX_DEPTH || self.frames.len() > 4 * MAX_DEPTH {
                    return Err(Error::runtime("stack overflow"));
                }
                if let Some(r) = self.call_at(host, sp, argc as usize, false)? {
                    self.stack.truncate(sp);
                    self.stack.push(r);
                }
            }
            Op::Return | Op::ReturnNull => {
                let v = if op == Op::Return { self.pop() } else { Value::Null };
                let f = self.frames.pop().expect("frame");
                self.traps.truncate(f.traps_base);
                if let Some(g) = f.generator {
                    self.heap.generator_mut(g).state = GenState::Dead;
                }
                self.stack.truncate(f.floor);
                if self.frames.len() == stop {
                    return Ok(Some(v));
                }
                self.stack.push(v);
            }
            Op::Yield | Op::YieldNull => {
                let v = if op == Op::Yield { self.pop() } else { Value::Null };
                let f = self.frames.pop().expect("frame");
                let Some(g) = f.generator else {
                    return Err(Error::runtime("trying to yield a 'closure', only genenerator can be yielded"));
                };
                self.traps.truncate(f.traps_base);
                let saved = self.stack.split_off(f.base);
                let generator = self.heap.generator_mut(g);
                generator.stack = saved;
                generator.ip = f.ip;
                generator.varargs = f.varargs;
                generator.state = GenState::Suspended;
                if self.frames.len() == stop {
                    return Ok(Some(v));
                }
                self.stack.push(v);
            }
            Op::NewTable(n) => {
                let t = self.heap.alloc(Object::Table(Table { map: SqTable::with_capacity(n as usize), delegate: None }));
                self.stack.push(Value::Table(t));
            }
            Op::NewArray(n) => {
                let a = self.heap.alloc(Object::Array(Vec::with_capacity(n as usize)));
                self.stack.push(Value::Array(a));
            }
            Op::ArrayAppend => {
                let v = self.pop();
                if let Value::Array(a) = self.top() {
                    let a = *a;
                    self.heap.array_mut(a).push(v);
                }
            }
            Op::TableSlot => {
                let v = self.pop();
                let k = self.pop();
                if k.is_null() {
                    return Err(Error::runtime("null cannot be used as index"));
                }
                if let Value::Table(t) = self.top() {
                    let t = *t;
                    self.heap.table_mut(t).map.new_slot(k, v);
                }
            }
            Op::Closure { proto, defaults } => {
                let defaults = self.stack.split_off(self.stack.len() - defaults as usize);
                let parent = self.frames[fi].closure;
                let p = self.frames[fi].proto.protos[proto as usize].clone();
                let outers = p
                    .outers
                    .iter()
                    .map(|o| match o {
                        OuterSource::Local(s) => self.stack[base + *s as usize].clone(),
                        OuterSource::Outer(i) => self.heap.closure(parent).outers[*i as usize].clone(),
                    })
                    .collect();
                let c = self.heap.alloc(Object::Closure(Closure { proto: p, outers, defaults, env: None }));
                self.stack.push(Value::Closure(c));
            }
            Op::NewClass { base: has_base, attrs } => {
                let attrs = if attrs { self.pop() } else { Value::Null };
                let base_class = if has_base {
                    match self.pop() {
                        Value::Class(c) => Some(c),
                        other => return Err(Error::runtime(format!("trying to inherit from a {}", other.type_name()))),
                    }
                } else {
                    None
                };
                let class = self.new_class(base_class, 0);
                if let Value::Class(c) = &class {
                    self.heap.class_mut(*c).attrs = attrs.clone();
                }
                if let Some(b) = base_class
                    && let Some(m) = self.heap.class(b).metamethod(b"_inherited").cloned()
                {
                    self.call(host, &m, &Value::Class(b), &[class.clone(), attrs])?;
                }
                self.stack.push(class);
            }
            Op::ClassMember { is_static, attrs } => {
                let attrs = if attrs { self.pop() } else { Value::Null };
                let val = self.pop();
                let key = self.pop();
                if let Value::Class(c) = self.top() {
                    let c = *c;
                    self.class_new_slot(c, key, val, is_static, attrs)?;
                }
            }
            Op::ForeachInit(slot) => {
                self.stack[base + slot as usize + 1] = Value::Null;
            }
            Op::ForeachNext { base: slot, key, value, exit } => {
                let container = self.stack[base + slot as usize].clone();
                let iter = self.stack[base + slot as usize + 1].clone();
                let pos = iter.as_i32().unwrap_or(0).max(0) as usize;
                let next: Option<(Value, Value, Value)> = match &container {
                    Value::Table(t) => {
                        self.heap.table(*t).map.next(pos).map(|(n, k, v)| (Value::Int(n as i32), k.clone(), v.clone()))
                    }
                    Value::Array(a) => {
                        let arr = self.heap.array(*a);
                        (pos < arr.len()).then(|| (Value::Int(pos as i32 + 1), Value::Int(pos as i32), arr[pos].clone()))
                    }
                    Value::Str(s) => (pos < s.len())
                        .then(|| (Value::Int(pos as i32 + 1), Value::Int(pos as i32), Value::Int(s.bytes()[pos] as i8 as i32))),
                    Value::Class(c) => {
                        let class = self.heap.class(*c);
                        class.members.get(pos).map(|(k, kind, i)| {
                            let v = match kind {
                                MemberKind::Field => class.fields[*i].0.clone(),
                                MemberKind::Method => class.methods[*i].0.clone(),
                            };
                            (Value::Int(pos as i32 + 1), k.clone(), v)
                        })
                    }
                    Value::Instance(_) => match self.metamethod(&container, b"_nexti") {
                        Some(m) => {
                            let k = self.call(host, &m, &container, std::slice::from_ref(&iter))?;
                            if k.is_null() {
                                None
                            } else {
                                match self.get(host, &container, &k, false)? {
                                    Some(v) => Some((k.clone(), k, v)),
                                    None => return Err(Error::runtime("_nexti returned an invalid idx")),
                                }
                            }
                        }
                        None => {
                            let inst = self.heap.instance(container.obj().expect("instance"));
                            let class = self.heap.class(inst.class);
                            class.members.get(pos).map(|(k, kind, i)| {
                                let v = match kind {
                                    MemberKind::Field => inst.fields[*i].clone(),
                                    MemberKind::Method => class.methods[*i].0.clone(),
                                };
                                (Value::Int(pos as i32 + 1), k.clone(), v)
                            })
                        }
                    },
                    Value::Generator(g) => {
                        let g = *g;
                        if self.heap.generator_mut(g).state == GenState::Dead {
                            None
                        } else {
                            let idx = match iter {
                                Value::Int(i) => i + 1,
                                _ => 0,
                            };
                            let v = self.resume(host, g)?;
                            if self.heap.generator_mut(g).state == GenState::Dead {
                                None
                            } else {
                                Some((Value::Int(idx), Value::Int(idx), v))
                            }
                        }
                    }
                    other => return Err(Error::runtime(format!("cannot iterate {}", other.type_name()))),
                };
                match next {
                    Some((n, k, v)) => {
                        self.stack[base + slot as usize + 1] = n;
                        if key != u16::MAX {
                            self.stack[base + key as usize] = k;
                        }
                        self.stack[base + value as usize] = v;
                    }
                    None => self.frames[fi].ip = exit as usize,
                }
            }
            Op::PushTrap(target) => {
                let stack_len = self.stack.len();
                self.traps.push(Trap { frame: fi, ip: target as usize, stack_len });
            }
            Op::PopTrap(n) => {
                let keep = self.traps.len().saturating_sub(n as usize);
                self.traps.truncate(keep);
            }
            Op::Throw => {
                let v = self.pop();
                let message = match &v {
                    Value::Str(s) => s.to_string_lossy(),
                    other => self.to_string(host, other)?.to_string_lossy(),
                };
                return Err(Error::thrown(v, message));
            }
            Op::Delegate => {
                let table = self.pop();
                let parent = self.pop();
                let Value::Table(t) = table else {
                    return Err(Error::runtime(format!("delegate: {} is not a table", table.type_name())));
                };
                let d = match parent {
                    Value::Table(p) => Some(p),
                    Value::Null => None,
                    other => return Err(Error::runtime(format!("delegate: {} is not a table", other.type_name()))),
                };
                self.heap.table_mut(t).delegate = d;
                self.stack.push(Value::Table(t));
            }
        }
        Ok(None)
    }

    // ----------------------------------------------------------- utilities

    /// Next pseudo-random number (the C library's `rand` sequence shape:
    /// 15-bit results).
    pub fn rand(&mut self) -> i32 {
        self.rand_state = self.rand_state.wrapping_mul(214013).wrapping_add(2531011);
        ((self.rand_state >> 16) & 0x7fff) as i32
    }

    /// Text of a value for printing (tostring rules, metamethods included).
    pub fn display(&mut self, host: &mut dyn Any, v: &Value) -> String {
        self.to_string(host, v).map(|s| s.to_string_lossy()).unwrap_or_default()
    }
}
