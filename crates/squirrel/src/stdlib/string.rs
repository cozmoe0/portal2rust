//! String library: `format`, trimming, `split` and `regexp`.

use super::regex::Regex;
use super::{arg, arg_float, arg_int, arg_str};
use crate::error::{Error, Result};
use crate::format::{Spec, format_float, format_int};
use crate::value::{ObjRef, SqStr, Value};
use crate::vm::{Ctx, Vm};

/// Type tag of the `regexp` class; instances keep their pattern in field 0.
pub const REGEXP_TAG: u32 = 0x5245_4758;

pub fn register(vm: &mut Vm, root: &Value) {
    vm.set_native(root, "format", format, -2);
    vm.set_native(root, "strip", strip, 2);
    vm.set_native(root, "lstrip", lstrip, 2);
    vm.set_native(root, "rstrip", rstrip, 2);
    vm.set_native(root, "split", split, 3);
    let class = vm.new_class(None, REGEXP_TAG);
    let Value::Class(c) = class else { unreachable!() };
    let _ = vm.class_new_slot(c, Value::str("@pattern"), Value::Null, false, Value::Null);
    for (name, f, n) in [
        ("constructor", regexp_ctor as crate::value::NativeFn, 2),
        ("match", regexp_match, 2),
        ("search", regexp_search, -2),
        ("capture", regexp_capture, -2),
        ("subexpcount", regexp_subexpcount, 1),
    ] {
        let native = vm.new_native(name, f, n, Value::Null);
        let _ = vm.class_new_slot(c, Value::str(name), native, false, Value::Null);
    }
    vm.raw_set(root, Value::str("regexp"), class);
}

/// printf-style formatting of script values.
pub fn format(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let fmt = arg_str(args, 1)?;
    let fmt = fmt.bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut next = 2;
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            out.push(fmt[i]);
            i += 1;
            continue;
        }
        if fmt.get(i + 1) == Some(&b'%') {
            out.push(b'%');
            i += 2;
            continue;
        }
        let Some((spec, used)) = Spec::parse(&fmt[i + 1..]) else {
            return Err(Error::runtime("invalid format"));
        };
        i += 1 + used;
        if next >= args.len() {
            return Err(Error::runtime("not enough paramters for the given format string"));
        }
        let text = match spec.conv {
            'd' | 'i' | 'c' | 'o' | 'u' | 'x' | 'X' => format_int(arg_int(args, next)? as i64, &spec),
            'f' | 'g' | 'G' | 'e' | 'E' => format_float(arg_float(args, next)? as f64, &spec),
            's' => {
                let Value::Str(s) = arg(args, next) else {
                    return Err(Error::runtime("string expected for the specified format"));
                };
                let mut body: Vec<u8> = s.bytes().to_vec();
                if let Some(p) = spec.precision {
                    body.truncate(p);
                }
                let fill = spec.width.saturating_sub(body.len());
                let mut padded = Vec::new();
                if !spec.left {
                    padded.extend(std::iter::repeat_n(b' ', fill));
                }
                padded.extend_from_slice(&body);
                if spec.left {
                    padded.extend(std::iter::repeat_n(b' ', fill));
                }
                out.extend_from_slice(&padded);
                next += 1;
                continue;
            }
            _ => return Err(Error::runtime("invalid format")),
        };
        out.extend(text.chars().map(|c| c as u8));
        next += 1;
    }
    let _ = ctx;
    Ok(Value::Str(SqStr::new(&out)))
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn strip(_ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let s = arg_str(args, 1)?;
    let b = s.bytes();
    let start = b.iter().position(|c| !is_space(*c)).unwrap_or(b.len());
    let end = b.iter().rposition(|c| !is_space(*c)).map(|e| e + 1).unwrap_or(start);
    Ok(Value::Str(SqStr::new(&b[start..end.max(start)])))
}

fn lstrip(_ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let s = arg_str(args, 1)?;
    let b = s.bytes();
    let start = b.iter().position(|c| !is_space(*c)).unwrap_or(b.len());
    Ok(Value::Str(SqStr::new(&b[start..])))
}

fn rstrip(_ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let s = arg_str(args, 1)?;
    let b = s.bytes();
    let end = b.iter().rposition(|c| !is_space(*c)).map(|e| e + 1).unwrap_or(0);
    Ok(Value::Str(SqStr::new(&b[..end])))
}

/// Splits at any of the separator characters, dropping empty pieces.
fn split(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let s = arg_str(args, 1)?;
    let seps = arg_str(args, 2)?;
    let items: Vec<Value> = s
        .bytes()
        .split(|c| seps.bytes().contains(c))
        .filter(|p| !p.is_empty())
        .map(|p| Value::Str(SqStr::new(p)))
        .collect();
    Ok(ctx.vm.new_array(items))
}

fn this_regex(ctx: &mut Ctx, args: &[Value]) -> Result<Regex> {
    let Value::Instance(i) = arg(args, 0) else { return Err(Error::runtime("invalid regexp")) };
    let inst = ctx.vm.heap.instance(*i);
    match inst.fields.first() {
        Some(Value::Str(p)) => Regex::new(p.bytes()),
        _ => Err(Error::runtime("invalid regexp")),
    }
}

fn regexp_ctor(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let pattern = arg_str(args, 1)?;
    Regex::new(pattern.bytes())?;
    if let Value::Instance(i) = arg(args, 0) {
        ctx.vm.heap.instance_mut(*i).fields[0] = Value::Str(pattern);
    }
    Ok(Value::Null)
}

fn regexp_match(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let re = this_regex(ctx, args)?;
    let s = arg_str(args, 1)?;
    Ok(Value::Bool(re.full_match(s.bytes())))
}

fn range_table(ctx: &mut Ctx, begin: usize, end: usize) -> Value {
    let t = ctx.vm.new_table();
    ctx.vm.raw_set(&t, Value::str("begin"), Value::Int(begin as i32));
    ctx.vm.raw_set(&t, Value::str("end"), Value::Int(end as i32));
    t
}

fn regexp_search(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let re = this_regex(ctx, args)?;
    let s = arg_str(args, 1)?;
    let start = if args.len() > 2 { arg_int(args, 2)?.max(0) as usize } else { 0 };
    Ok(match re.search(s.bytes(), start) {
        Some(groups) => {
            let (b, e) = groups[0].unwrap_or((0, 0));
            range_table(ctx, b, e)
        }
        None => Value::Null,
    })
}

fn regexp_capture(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let re = this_regex(ctx, args)?;
    let s = arg_str(args, 1)?;
    let start = if args.len() > 2 { arg_int(args, 2)?.max(0) as usize } else { 0 };
    Ok(match re.search(s.bytes(), start) {
        Some(groups) => {
            let items: Vec<Value> = groups
                .iter()
                .map(|g| {
                    let (b, e) = g.unwrap_or((0, 0));
                    range_table(ctx, b, e)
                })
                .collect();
            ctx.vm.new_array(items)
        }
        None => Value::Null,
    })
}

fn regexp_subexpcount(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let re = this_regex(ctx, args)?;
    Ok(Value::Int(re.groups() as i32))
}

#[allow(dead_code)]
fn _class_of(_r: ObjRef) {}
