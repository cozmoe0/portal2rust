//! Base library: printing, assertions, the root table, compiling strings.

use super::{arg, arg_int, arg_str};
use crate::error::{Error, Result};
use crate::value::Value;
use crate::vm::{Ctx, Vm};

pub fn register(vm: &mut Vm, root: &Value) {
    vm.set_native(root, "print", print, 2);
    vm.set_native(root, "assert", assert, 2);
    vm.set_native(root, "getroottable", getroottable, 1);
    vm.set_native(root, "getconsttable", getconsttable, 1);
    vm.set_native(root, "compilestring", compilestring, -2);
    vm.set_native(root, "array", array, -2);
    vm.set_native(root, "type", type_of, 2);
    vm.set_native(root, "seterrorhandler", seterrorhandler, 2);
    vm.set_native(root, "setdebughook", ignore, 2);
    vm.set_native(root, "enabledebuginfo", ignore, 2);
    vm.set_native(root, "getstackinfos", getstackinfos, 2);
    vm.set_native(root, "collectgarbage", collectgarbage, 1);
    vm.set_native(root, "dummy", ignore, 0);
}

fn ignore(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Null)
}

fn print(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let text = ctx.to_string(arg(args, 1))?.to_string_lossy();
    (ctx.vm.print)(ctx.host, &text);
    Ok(Value::Null)
}

fn assert(_ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    if arg(args, 1).truthy() { Ok(Value::Null) } else { Err(Error::runtime("assertion failed")) }
}

fn getroottable(ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(ctx.vm.root_value())
}

/// A table with the compile-time constants (changes do not affect them).
fn getconsttable(ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    let entries: Vec<(Value, Value)> = ctx.vm.consts.iter().map(|(k, v)| (Value::Str(crate::value::SqStr::new(k)), v.clone())).collect();
    let t = ctx.vm.new_table();
    for (k, v) in entries {
        ctx.vm.raw_set(&t, k, v);
    }
    Ok(t)
}

fn compilestring(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let src = arg_str(args, 1)?;
    let name = match arg(args, 2) {
        Value::Str(s) => s.to_string_lossy(),
        _ => "unnamedbuffer".to_owned(),
    };
    ctx.vm.compile(src.bytes(), &name)
}

fn array(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let n = arg_int(args, 1)?;
    if n < 0 {
        return Err(Error::runtime("negative size"));
    }
    let fill = arg(args, 2).clone();
    Ok(ctx.vm.new_array(vec![fill; n as usize]))
}

fn type_of(_ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    Ok(Value::str(arg(args, 1).type_name()))
}

fn seterrorhandler(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    ctx.vm.error_handler = arg(args, 1).clone();
    Ok(Value::Null)
}

fn getstackinfos(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Null)
}

fn collectgarbage(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    // Collection only runs between host calls; report nothing freed.
    Ok(Value::Int(0))
}
