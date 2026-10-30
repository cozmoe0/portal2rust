//! Default delegates: the methods available on every value of a type
//! (`"x".len()`, `a.push(v)`, `f.call(this)`, `(1).tofloat()`, ...).

use super::{arg, arg_int, arg_str};
use crate::error::{Error, Result};
use crate::value::*;
use crate::vm::{Ctx, Vm};

pub fn register(vm: &mut Vm) {
    let d = vm.delegates;
    let set = |vm: &mut Vm, t: ObjRef, name: &str, f: NativeFn, n: i32| vm.set_native(&Value::Table(t), name, f, n);
    {
        let t = d.number;
        set(vm, t, "tointeger", to_integer, 1);
        set(vm, t, "tofloat", to_float, 1);
        set(vm, t, "tostring", to_string, 1);
        set(vm, t, "tochar", to_char, 1);
        set(vm, t, "weakref", weakref, 1);
    }
    set(vm, d.string, "len", len, 1);
    set(vm, d.string, "tointeger", to_integer, 1);
    set(vm, d.string, "tofloat", to_float, 1);
    set(vm, d.string, "tostring", to_string, 1);
    set(vm, d.string, "slice", slice, -2);
    set(vm, d.string, "find", find, -2);
    set(vm, d.string, "tolower", tolower, 1);
    set(vm, d.string, "toupper", toupper, 1);
    set(vm, d.string, "weakref", weakref, 1);

    set(vm, d.table, "len", len, 1);
    set(vm, d.table, "rawget", rawget, 2);
    set(vm, d.table, "rawset", rawset, 3);
    set(vm, d.table, "rawdelete", rawdelete, 2);
    set(vm, d.table, "rawin", rawin, 2);
    set(vm, d.table, "weakref", weakref, 1);
    set(vm, d.table, "tostring", to_string, 1);
    set(vm, d.table, "clear", clear, 1);
    set(vm, d.table, "setdelegate", setdelegate, 2);
    set(vm, d.table, "getdelegate", getdelegate, 1);

    set(vm, d.array, "len", len, 1);
    set(vm, d.array, "append", push, 2);
    set(vm, d.array, "push", push, 2);
    set(vm, d.array, "extend", extend, 2);
    set(vm, d.array, "pop", pop, 1);
    set(vm, d.array, "top", top, 1);
    set(vm, d.array, "insert", insert, 3);
    set(vm, d.array, "remove", remove, 2);
    set(vm, d.array, "resize", resize, -2);
    set(vm, d.array, "sort", sort, -1);
    set(vm, d.array, "reverse", reverse, 1);
    set(vm, d.array, "slice", slice, -2);
    set(vm, d.array, "weakref", weakref, 1);
    set(vm, d.array, "tostring", to_string, 1);
    set(vm, d.array, "clear", clear, 1);

    set(vm, d.closure, "call", call, -1);
    set(vm, d.closure, "pcall", call, -1);
    set(vm, d.closure, "acall", acall, 2);
    set(vm, d.closure, "pacall", acall, 2);
    set(vm, d.closure, "weakref", weakref, 1);
    set(vm, d.closure, "tostring", to_string, 1);
    set(vm, d.closure, "bindenv", bindenv, 2);
    set(vm, d.closure, "getinfos", getinfos, 1);

    set(vm, d.generator, "getstatus", getstatus, 1);
    set(vm, d.generator, "weakref", weakref, 1);
    set(vm, d.generator, "tostring", to_string, 1);

    set(vm, d.class, "getattributes", getattributes, 2);
    set(vm, d.class, "setattributes", setattributes, 3);
    set(vm, d.class, "rawin", rawin, 2);
    set(vm, d.class, "weakref", weakref, 1);
    set(vm, d.class, "tostring", to_string, 1);
    set(vm, d.class, "instance", instance, 1);

    set(vm, d.instance, "getclass", getclass, 1);
    set(vm, d.instance, "rawin", rawin, 2);
    set(vm, d.instance, "weakref", weakref, 1);
    set(vm, d.instance, "tostring", to_string, 1);

    set(vm, d.weakref, "ref", weak_ref, 1);
    set(vm, d.weakref, "weakref", weakref, 1);
    set(vm, d.weakref, "tostring", to_string, 1);
}

/// The original's string to number conversion: a float when the text has
/// a '.', else a base 10 integer; trailing junk is ignored.
pub fn str_to_number(s: &[u8]) -> Option<Value> {
    let text: String = s.iter().map(|&b| b as char).collect();
    let t = text.trim_start();
    if t.contains('.') {
        let end = t
            .char_indices()
            .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || ((c == '-' || c == '+') && i == 0) || c == 'e' || c == 'E'))
            .map(|(i, _)| i)
            .unwrap_or(t.len());
        let mut num = &t[..end];
        while !num.is_empty() && num.parse::<f64>().is_err() {
            num = &num[..num.len() - 1];
        }
        return num.parse::<f64>().ok().map(|v| Value::Float(v as f32));
    }
    let bytes = t.as_bytes();
    let mut i = 0;
    let neg = matches!(bytes.first(), Some(b'-'));
    if matches!(bytes.first(), Some(b'-' | b'+')) {
        i = 1;
    }
    let start = i;
    let mut v: i64 = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        v = (v * 10 + (bytes[i] - b'0') as i64).min(i64::from(u32::MAX) + 1);
        i += 1;
    }
    if i == start {
        return None;
    }
    let v = if neg { -v } else { v };
    Some(Value::Int(v.clamp(i32::MIN as i64, i32::MAX as i64) as i32))
}

fn to_integer(_ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    Ok(match arg(args, 0) {
        Value::Int(i) => Value::Int(*i),
        Value::Float(f) => Value::Int(*f as i32),
        Value::Bool(b) => Value::Int(*b as i32),
        Value::Str(s) => match str_to_number(s.bytes()) {
            Some(Value::Float(f)) => Value::Int(f as i32),
            Some(v) => v,
            None => return Err(Error::runtime("cannot convert the string")),
        },
        _ => Value::Null,
    })
}

fn to_float(_ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    Ok(match arg(args, 0) {
        Value::Int(i) => Value::Float(*i as f32),
        Value::Float(f) => Value::Float(*f),
        Value::Bool(b) => Value::Float(*b as i32 as f32),
        Value::Str(s) => match str_to_number(s.bytes()) {
            Some(Value::Int(i)) => Value::Float(i as f32),
            Some(v) => v,
            None => return Err(Error::runtime("cannot convert the string")),
        },
        _ => Value::Null,
    })
}

fn to_string(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    Ok(Value::Str(ctx.to_string(arg(args, 0))?))
}

fn to_char(_ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let c = arg_int(args, 0)?;
    Ok(Value::Str(SqStr::new(&[c as u8])))
}

fn weakref(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let v = arg(args, 0).clone();
    if v.obj().is_none() || matches!(v, Value::WeakRef(_)) {
        return Ok(v);
    }
    Ok(Value::WeakRef(ctx.vm.heap.alloc(Object::WeakRef(v))))
}

fn weak_ref(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    match arg(args, 0) {
        Value::WeakRef(w) => Ok(ctx.vm.heap.weak_target(*w)),
        other => Ok(other.clone()),
    }
}

fn len(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    Ok(Value::Int(match arg(args, 0) {
        Value::Str(s) => s.len() as i32,
        Value::Table(t) => ctx.vm.heap.table(*t).map.len() as i32,
        Value::Array(a) => ctx.vm.heap.array(*a).len() as i32,
        other => return Err(super::wrong_type(other.type_name())),
    }))
}

/// Resolves slice bounds with negative values counting from the end.
fn bounds(len: usize, args: &[Value]) -> Result<(usize, usize)> {
    let len = len as i32;
    let mut s = arg_int(args, 1)?;
    let mut e = if args.len() > 2 && !arg(args, 2).is_null() { arg_int(args, 2)? } else { len };
    if s < 0 {
        s += len;
    }
    if e < 0 {
        e += len;
    }
    if s > e || s < 0 || e > len {
        return Err(Error::runtime("wrong indexes"));
    }
    Ok((s as usize, e as usize))
}

fn slice(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    match arg(args, 0) {
        Value::Str(s) => {
            let (a, b) = bounds(s.len(), args)?;
            Ok(Value::Str(SqStr::new(&s.bytes()[a..b])))
        }
        Value::Array(arr) => {
            let items = ctx.vm.heap.array(*arr).clone();
            let (a, b) = bounds(items.len(), args)?;
            Ok(ctx.vm.new_array(items[a..b].to_vec()))
        }
        other => Err(super::wrong_type(other.type_name())),
    }
}

fn find(_ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let Value::Str(s) = arg(args, 0) else { return Ok(Value::Null) };
    let needle = arg_str(args, 1)?;
    let start = if args.len() > 2 { arg_int(args, 2)?.max(0) as usize } else { 0 };
    if start > s.len() {
        return Ok(Value::Null);
    }
    let hay = &s.bytes()[start..];
    let n = needle.bytes();
    if n.is_empty() {
        return Ok(Value::Int(start as i32));
    }
    Ok(hay.windows(n.len()).position(|w| w == n).map(|i| Value::Int((start + i) as i32)).unwrap_or(Value::Null))
}

fn tolower(_ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let s = arg_str(args, 0)?;
    Ok(Value::Str(SqStr::new(&s.bytes().to_ascii_lowercase())))
}

fn toupper(_ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let s = arg_str(args, 0)?;
    Ok(Value::Str(SqStr::new(&s.bytes().to_ascii_uppercase())))
}

fn rawget(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    match ctx.vm.get(ctx.host, arg(args, 0), arg(args, 1), true)? {
        Some(v) => Ok(v),
        None => Err(Error::runtime("the index doesn't exist")),
    }
}

fn rawset(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (t, k, v) = (arg(args, 0).clone(), arg(args, 1).clone(), arg(args, 2).clone());
    if k.is_null() {
        return Err(Error::runtime("null cannot be used as index"));
    }
    if let Value::Table(r) = &t {
        ctx.vm.heap.table_mut(*r).map.new_slot(k, v);
    }
    Ok(t)
}

fn rawdelete(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let Value::Table(t) = arg(args, 0) else { return Ok(Value::Null) };
    Ok(ctx.vm.heap.table_mut(*t).map.remove(arg(args, 1)).unwrap_or_default())
}

fn rawin(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let key = arg(args, 1);
    Ok(Value::Bool(match arg(args, 0) {
        Value::Table(t) => ctx.vm.heap.table(*t).map.contains(key),
        Value::Class(c) => ctx.vm.heap.class(*c).find(key).is_some(),
        Value::Instance(i) => {
            let class = ctx.vm.heap.instance(*i).class;
            ctx.vm.heap.class(class).find(key).is_some()
        }
        _ => false,
    }))
}

fn clear(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    match arg(args, 0) {
        Value::Table(t) => ctx.vm.heap.table_mut(*t).map.clear(),
        Value::Array(a) => ctx.vm.heap.array_mut(*a).clear(),
        _ => {}
    }
    Ok(arg(args, 0).clone())
}

fn setdelegate(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let Value::Table(t) = arg(args, 0) else { return Ok(Value::Null) };
    let d = match arg(args, 1) {
        Value::Table(d) => {
            // Refuse cycles.
            let mut cur = Some(*d);
            while let Some(c) = cur {
                if c == *t {
                    return Err(Error::runtime("delegate cycle detected"));
                }
                cur = ctx.vm.heap.table(c).delegate;
            }
            Some(*d)
        }
        Value::Null => None,
        other => return Err(super::wrong_type(other.type_name())),
    };
    ctx.vm.heap.table_mut(*t).delegate = d;
    Ok(arg(args, 0).clone())
}

fn getdelegate(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let Value::Table(t) = arg(args, 0) else { return Ok(Value::Null) };
    Ok(ctx.vm.heap.table(*t).delegate.map(Value::Table).unwrap_or_default())
}

fn push(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    if let Value::Array(a) = arg(args, 0) {
        ctx.vm.heap.array_mut(*a).push(arg(args, 1).clone());
    }
    Ok(Value::Null)
}

fn extend(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (Value::Array(a), Value::Array(b)) = (arg(args, 0), arg(args, 1)) else {
        return Err(super::wrong_type(arg(args, 1).type_name()));
    };
    let items = ctx.vm.heap.array(*b).clone();
    ctx.vm.heap.array_mut(*a).extend(items);
    Ok(arg(args, 0).clone())
}

fn pop(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let Value::Array(a) = arg(args, 0) else { return Ok(Value::Null) };
    ctx.vm.heap.array_mut(*a).pop().ok_or_else(|| Error::runtime("empty array"))
}

fn top(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let Value::Array(a) = arg(args, 0) else { return Ok(Value::Null) };
    ctx.vm.heap.array(*a).last().cloned().ok_or_else(|| Error::runtime("empty array"))
}

fn insert(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let Value::Array(a) = arg(args, 0) else { return Ok(Value::Null) };
    let idx = arg_int(args, 1)?;
    let arr = ctx.vm.heap.array_mut(*a);
    if idx < 0 || idx as usize > arr.len() {
        return Err(Error::runtime("index out of range"));
    }
    arr.insert(idx as usize, arg(args, 2).clone());
    Ok(Value::Null)
}

fn remove(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let Value::Array(a) = arg(args, 0) else { return Ok(Value::Null) };
    let idx = arg_int(args, 1)?;
    let arr = ctx.vm.heap.array_mut(*a);
    if idx < 0 || idx as usize >= arr.len() {
        return Err(Error::runtime("idx out of range"));
    }
    Ok(arr.remove(idx as usize))
}

fn resize(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let Value::Array(a) = arg(args, 0) else { return Ok(Value::Null) };
    let n = arg_int(args, 1)?;
    if n < 0 {
        return Err(Error::runtime("negative size"));
    }
    let fill = arg(args, 2).clone();
    ctx.vm.heap.array_mut(*a).resize(n as usize, fill);
    Ok(Value::Null)
}

fn reverse(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    if let Value::Array(a) = arg(args, 0) {
        ctx.vm.heap.array_mut(*a).reverse();
    }
    Ok(Value::Null)
}

fn sort(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let Value::Array(a) = arg(args, 0) else { return Ok(Value::Null) };
    let mut items = ctx.vm.heap.array(*a).clone();
    let func = arg(args, 1).clone();
    // Merge sort with a fallible comparison.
    let mut err = None;
    let mut cmp = |x: &Value, y: &Value| -> std::cmp::Ordering {
        if err.is_some() {
            return std::cmp::Ordering::Equal;
        }
        let r = if func.is_null() {
            ctx.vm.compare(ctx.host, x, y)
        } else {
            match ctx.vm.call(ctx.host, &func, &Value::Null, &[x.clone(), y.clone()]) {
                Ok(Value::Int(i)) => Ok(i),
                Ok(_) => Err(Error::runtime("numeric value expected as return value of the compare function")),
                Err(e) => Err(e),
            }
        };
        match r {
            Ok(i) => i.cmp(&0),
            Err(e) => {
                err = Some(e);
                std::cmp::Ordering::Equal
            }
        }
    };
    items.sort_by(|x, y| cmp(x, y));
    if let Some(e) = err {
        return Err(e);
    }
    *ctx.vm.heap.array_mut(*a) = items;
    Ok(Value::Null)
}

fn call(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let f = arg(args, 0).clone();
    let this = arg(args, 1).clone();
    let rest = if args.len() > 2 { args[2..].to_vec() } else { Vec::new() };
    ctx.call(&f, &this, &rest)
}

fn acall(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let f = arg(args, 0).clone();
    let Value::Array(a) = arg(args, 1) else { return Err(super::wrong_type(arg(args, 1).type_name())) };
    let items = ctx.vm.heap.array(*a).clone();
    let Some((this, rest)) = items.split_first() else { return Err(Error::runtime("wrong number of parameters")) };
    ctx.call(&f, this, rest)
}

fn bindenv(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let env = arg(args, 1).clone();
    let env = match &env {
        v if v.obj().is_some() && !matches!(v, Value::WeakRef(_)) => Value::WeakRef(ctx.vm.heap.alloc(Object::WeakRef(env.clone()))),
        _ => env,
    };
    match arg(args, 0) {
        Value::Closure(c) => {
            let mut closure = ctx.vm.heap.closure(*c).clone();
            closure.env = Some(env);
            Ok(Value::Closure(ctx.vm.heap.alloc(Object::Closure(closure))))
        }
        Value::Native(n) => {
            let mut native = ctx.vm.heap.native(*n).clone();
            native.env = Some(env);
            Ok(Value::Native(ctx.vm.heap.alloc(Object::Native(native))))
        }
        other => Err(super::wrong_type(other.type_name())),
    }
}

fn getinfos(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let t = ctx.vm.new_table();
    match arg(args, 0) {
        Value::Closure(c) => {
            let proto = ctx.vm.heap.closure(*c).proto.clone();
            let mut params = vec![Value::str("this")];
            params.extend(proto.params.iter().cloned().map(Value::Str));
            if proto.varargs {
                params.push(Value::str("..."));
            }
            let params = ctx.vm.new_array(params);
            ctx.vm.raw_set(&t, Value::str("native"), Value::Bool(false));
            ctx.vm.raw_set(&t, Value::str("name"), Value::Str(proto.name.clone()));
            ctx.vm.raw_set(&t, Value::str("src"), Value::Str(proto.source.clone()));
            ctx.vm.raw_set(&t, Value::str("parameters"), params);
            ctx.vm.raw_set(&t, Value::str("varargs"), Value::Int(proto.varargs as i32));
        }
        Value::Native(n) => {
            let native = ctx.vm.heap.native(*n).clone();
            ctx.vm.raw_set(&t, Value::str("native"), Value::Bool(true));
            ctx.vm.raw_set(&t, Value::str("name"), Value::Str(native.name));
            ctx.vm.raw_set(&t, Value::str("paramscheck"), Value::Int(native.nparams));
        }
        _ => {}
    }
    Ok(t)
}

fn getstatus(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let Value::Generator(g) = arg(args, 0) else { return Ok(Value::Null) };
    Ok(Value::str(match ctx.vm.heap.generator_mut(*g).state {
        GenState::Running => "running",
        GenState::Suspended => "suspended",
        GenState::Dead => "dead",
    }))
}

fn getattributes(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let Value::Class(c) = arg(args, 0) else { return Ok(Value::Null) };
    let class = ctx.vm.heap.class(*c);
    let key = arg(args, 1);
    if key.is_null() {
        return Ok(class.attrs.clone());
    }
    match class.find(key) {
        Some((MemberKind::Field, i)) => Ok(class.fields[i].1.clone()),
        Some((MemberKind::Method, i)) => Ok(class.methods[i].1.clone()),
        None => Err(Error::runtime("wrong index")),
    }
}

fn setattributes(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let Value::Class(c) = arg(args, 0) else { return Ok(Value::Null) };
    let (key, val) = (arg(args, 1).clone(), arg(args, 2).clone());
    let class = ctx.vm.heap.class_mut(*c);
    if key.is_null() {
        return Ok(std::mem::replace(&mut class.attrs, val));
    }
    match class.find(&key) {
        Some((MemberKind::Field, i)) => Ok(std::mem::replace(&mut class.fields[i].1, val)),
        Some((MemberKind::Method, i)) => Ok(std::mem::replace(&mut class.methods[i].1, val)),
        None => Err(Error::runtime("wrong index")),
    }
}

fn instance(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let Value::Class(c) = arg(args, 0) else { return Ok(Value::Null) };
    Ok(ctx.vm.new_instance(*c, 0))
}

fn getclass(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let Value::Instance(i) = arg(args, 0) else { return Ok(Value::Null) };
    Ok(Value::Class(ctx.vm.heap.instance(*i).class))
}
