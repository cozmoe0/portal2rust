//! Math library.

use super::{arg, arg_float, arg_int};
use crate::error::Result;
use crate::value::Value;
use crate::vm::{Ctx, Vm};

pub fn register(vm: &mut Vm, root: &Value) {
    macro_rules! unary {
        ($($name:literal => $f:expr),* $(,)?) => {
            $(
                vm.set_native(root, $name, |_ctx, args| {
                    let x = arg_float(args, 1)?;
                    let f: fn(f32) -> f32 = $f;
                    Ok(Value::Float(f(x)))
                }, 2);
            )*
        };
    }
    unary! {
        "sqrt" => f32::sqrt,
        "sin" => f32::sin,
        "cos" => f32::cos,
        "tan" => f32::tan,
        "asin" => f32::asin,
        "acos" => f32::acos,
        "atan" => f32::atan,
        "log" => f32::ln,
        "log10" => f32::log10,
        "exp" => f32::exp,
        "floor" => f32::floor,
        "ceil" => f32::ceil,
        "fabs" => f32::abs,
    }
    vm.set_native(root, "atan2", atan2, 3);
    vm.set_native(root, "pow", pow, 3);
    vm.set_native(root, "abs", abs, 2);
    vm.set_native(root, "rand", rand, 1);
    vm.set_native(root, "srand", srand, 2);
    vm.raw_set(root, Value::str("RAND_MAX"), Value::Int(0x7fff));
    vm.raw_set(root, Value::str("PI"), Value::Float(std::f32::consts::PI));
}

fn atan2(_ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    Ok(Value::Float(arg_float(args, 1)?.atan2(arg_float(args, 2)?)))
}

fn pow(_ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    Ok(Value::Float(arg_float(args, 1)?.powf(arg_float(args, 2)?)))
}

fn abs(_ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    Ok(match arg(args, 1) {
        Value::Float(f) => Value::Int(f.abs() as i32),
        _ => Value::Int(arg_int(args, 1)?.wrapping_abs()),
    })
}

fn rand(ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Int(ctx.vm.rand()))
}

fn srand(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    ctx.vm.rand_state = arg_int(args, 1)? as u32;
    Ok(Value::Null)
}
