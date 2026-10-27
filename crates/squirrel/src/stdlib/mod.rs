//! Built-in functions: the base library, math, strings and the default
//! delegates (methods of strings, arrays, tables, functions, ...).

mod base;
mod delegates;
mod math;
mod regex;
mod string;

use crate::error::{Error, Result};
use crate::value::{SqStr, Value};
use crate::vm::Vm;

pub fn register(vm: &mut Vm) {
    let root = vm.root_value();
    base::register(vm, &root);
    math::register(vm, &root);
    string::register(vm, &root);
    delegates::register(vm);
}

pub(crate) fn wrong_type(what: &str) -> Error {
    Error::runtime(format!("parameter 1 has an invalid type '{what}'"))
}

pub(crate) fn arg<'a>(args: &'a [Value], i: usize) -> &'a Value {
    static NULL: Value = Value::Null;
    args.get(i).unwrap_or(&NULL)
}

pub(crate) fn arg_int(args: &[Value], i: usize) -> Result<i32> {
    match arg(args, i) {
        Value::Int(v) => Ok(*v),
        Value::Float(f) => Ok(*f as i32),
        Value::Bool(b) => Ok(*b as i32),
        other => Err(Error::runtime(format!("parameter {i} has an invalid type '{}' ; expected: 'integer'", other.type_name()))),
    }
}

pub(crate) fn arg_float(args: &[Value], i: usize) -> Result<f32> {
    match arg(args, i) {
        Value::Int(v) => Ok(*v as f32),
        Value::Float(f) => Ok(*f),
        other => Err(Error::runtime(format!("parameter {i} has an invalid type '{}' ; expected: 'float'", other.type_name()))),
    }
}

pub(crate) fn arg_str(args: &[Value], i: usize) -> Result<SqStr> {
    match arg(args, i) {
        Value::Str(s) => Ok(s.clone()),
        other => Err(Error::runtime(format!("parameter {i} has an invalid type '{}' ; expected: 'string'", other.type_name()))),
    }
}
