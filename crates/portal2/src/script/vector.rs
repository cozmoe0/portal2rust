//! The `Vector` script class: x, y, z with the usual arithmetic.

use bevy::prelude::Vec3;
use squirrel::value::NativeFn;
use squirrel::{Ctx, Error, ObjRef, Result, Value, Vm};

pub const VECTOR_TAG: u32 = 0x5645_4354;

fn component(v: &Value) -> f32 {
    v.as_f32().unwrap_or(0.0)
}

/// Reads a Vector instance.
pub fn read(vm: &Vm, v: &Value) -> Option<Vec3> {
    let Value::Instance(i) = v else { return None };
    let inst = vm.heap.instance(*i);
    if vm.heap.class(inst.class).type_tag != VECTOR_TAG {
        return None;
    }
    let f = &inst.fields;
    Some(Vec3::new(component(&f[0]), component(&f[1]), component(&f[2])))
}

pub fn make(vm: &mut Vm, class: ObjRef, v: Vec3) -> Value {
    let inst = vm.new_instance(class, 0);
    if let Value::Instance(i) = &inst {
        vm.heap.instance_mut(*i).fields = vec![Value::Float(v.x), Value::Float(v.y), Value::Float(v.z)];
    }
    inst
}

fn class_of(vm: &Vm, v: &Value) -> Option<ObjRef> {
    match v {
        Value::Instance(i) => Some(vm.heap.instance(*i).class),
        _ => None,
    }
}

fn this_vec(ctx: &Ctx, args: &[Value]) -> Result<Vec3> {
    read(ctx.vm, &args[0]).ok_or_else(|| Error::runtime("Vector method called on a non-vector"))
}

fn arg_vec(ctx: &Ctx, args: &[Value], i: usize) -> Result<Vec3> {
    args.get(i)
        .and_then(|v| read(ctx.vm, v))
        .ok_or_else(|| Error::runtime(format!("parameter {i} has an invalid type ; expected: 'Vector'")))
}

fn new_like(ctx: &mut Ctx, args: &[Value], v: Vec3) -> Result<Value> {
    let class = class_of(ctx.vm, &args[0]).ok_or_else(|| Error::runtime("bad vector"))?;
    Ok(make(ctx.vm, class, v))
}

fn constructor(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let get = |i: usize| args.get(i).map(component).unwrap_or(0.0);
    if let Value::Instance(i) = &args[0] {
        ctx.vm.heap.instance_mut(*i).fields = vec![Value::Float(get(1)), Value::Float(get(2)), Value::Float(get(3))];
    }
    Ok(Value::Null)
}

fn length(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    Ok(Value::Float(this_vec(ctx, args)?.length()))
}

fn length_sqr(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    Ok(Value::Float(this_vec(ctx, args)?.length_squared()))
}

fn length_2d(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    Ok(Value::Float(this_vec(ctx, args)?.truncate().length()))
}

fn length_2d_sqr(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    Ok(Value::Float(this_vec(ctx, args)?.truncate().length_squared()))
}

fn dot(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    Ok(Value::Float(this_vec(ctx, args)?.dot(arg_vec(ctx, args, 1)?)))
}

fn cross(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let v = this_vec(ctx, args)?.cross(arg_vec(ctx, args, 1)?);
    new_like(ctx, args, v)
}

/// Normalizes in place and returns the old length.
fn norm(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let v = this_vec(ctx, args)?;
    let len = v.length();
    let n = if len > 0.0 { v / len } else { Vec3::ZERO };
    if let Value::Instance(i) = &args[0] {
        ctx.vm.heap.instance_mut(*i).fields = vec![Value::Float(n.x), Value::Float(n.y), Value::Float(n.z)];
    }
    Ok(Value::Float(len))
}

fn to_kv_string(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let v = this_vec(ctx, args)?;
    Ok(Value::str(&format!("{:.6} {:.6} {:.6}", v.x, v.y, v.z)))
}

fn add(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let v = this_vec(ctx, args)? + arg_vec(ctx, args, 1)?;
    new_like(ctx, args, v)
}

fn sub(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let v = this_vec(ctx, args)? - arg_vec(ctx, args, 1)?;
    new_like(ctx, args, v)
}

fn mul(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let a = this_vec(ctx, args)?;
    let v = match args.get(1) {
        Some(s) if s.as_f32().is_some() => a * s.as_f32().unwrap_or(0.0),
        _ => a * arg_vec(ctx, args, 1)?,
    };
    new_like(ctx, args, v)
}

fn div(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let a = this_vec(ctx, args)?;
    let v = match args.get(1) {
        Some(s) if s.as_f32().is_some() => a / s.as_f32().unwrap_or(1.0),
        _ => a / arg_vec(ctx, args, 1)?,
    };
    new_like(ctx, args, v)
}

fn unm(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let v = -this_vec(ctx, args)?;
    new_like(ctx, args, v)
}

fn to_string(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let v = this_vec(ctx, args)?;
    Ok(Value::str(&format!("(vector : ({:.6}, {:.6}, {:.6}))", v.x, v.y, v.z)))
}

fn type_of(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::str("Vector"))
}

pub fn register(vm: &mut Vm) -> ObjRef {
    let Value::Class(class) = vm.new_class(None, VECTOR_TAG) else { unreachable!() };
    for f in ["x", "y", "z"] {
        let _ = vm.class_new_slot(class, Value::str(f), Value::Float(0.0), false, Value::Null);
    }
    let methods: &[(&str, NativeFn, i32)] = &[
        ("constructor", constructor, -1),
        ("Length", length, 1),
        ("LengthSqr", length_sqr, 1),
        ("Length2D", length_2d, 1),
        ("Length2DSqr", length_2d_sqr, 1),
        ("Dot", dot, 2),
        ("Cross", cross, 2),
        ("Norm", norm, 1),
        ("ToKVString", to_kv_string, 1),
        ("_add", add, 2),
        ("_sub", sub, 2),
        ("_mul", mul, 2),
        ("_div", div, 2),
        ("_unm", unm, 1),
        ("_tostring", to_string, 1),
        ("_typeof", type_of, 1),
    ];
    for (name, f, n) in methods {
        let native = vm.new_native(name, *f, *n, Value::Null);
        let _ = vm.class_new_slot(class, Value::str(name), native, false, Value::Null);
    }
    let root = vm.root_value();
    vm.raw_set(&root, Value::str("Vector"), Value::Class(class));
    class
}
