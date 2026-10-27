//! Compiles the syntax tree into stack bytecode, one prototype per function.
//!
//! Name resolution: locals, then captured locals of enclosing functions
//! (copied by value when the inner closure is created, as in Squirrel 2),
//! then constants, then slots of `this` with a fallback to the root table.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::*;
use crate::error::{Error, Result};
use crate::value::{SqStr, Value};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    Null,
    True,
    False,
    Int(i32),
    Float(f32),
    Const(u32),
    This,
    Root,
    Parent,
    /// [obj] -> [delegate or base class]
    ParentOf,
    Base,
    Vargc,
    /// [index] -> [vararg]
    Vargv,
    GetLocal(u16),
    /// Stores the top value, leaving it on the stack.
    SetLocal(u16),
    GetOuter(u16),
    SetOuter(u16),
    /// `this.name`, falling back to the root table.
    GetEnv(u32),
    SetEnv(u32),
    NewSlotEnv(u32),
    /// [obj, key] -> [value]
    Get,
    /// [obj] -> [obj.const]
    GetK(u32),
    /// [obj, key, value] -> [value]
    Set,
    NewSlot,
    /// [obj, key] -> [removed value]
    Delete,
    DeleteEnv(u32),
    /// [obj, key] -> [result] with `+= delta`; `post` keeps the old value.
    Inc { delta: i8, post: bool },
    IncLocal { slot: u16, delta: i8, post: bool },
    IncOuter { slot: u16, delta: i8, post: bool },
    IncEnv { name: u32, delta: i8, post: bool },
    Pop,
    Dup,
    /// [a, b] -> [a, b, a, b]
    Dup2,
    Swap,
    Unary(UnOp),
    Binary(BinOp),
    Jump(u32),
    JumpIfFalse(u32),
    JumpIfTrue(u32),
    /// Short circuits: jump keeping the value, or pop it and continue.
    AndJump(u32),
    OrJump(u32),
    /// [callee, this, args...] -> [result]
    Call(u16),
    Return,
    ReturnNull,
    Yield,
    YieldNull,
    NewTable(u32),
    NewArray(u32),
    /// [array, value] -> [array]
    ArrayAppend,
    /// [table, key, value] -> [table]
    TableSlot,
    /// Pops `defaults` default parameter values, pushes a closure.
    Closure { proto: u32, defaults: u16 },
    /// [base?, attrs?] -> [class]
    NewClass { base: bool, attrs: bool },
    /// [class, key, value, attrs?] -> [class]
    ClassMember { is_static: bool, attrs: bool },
    /// Container in `base`, iteration state in `base + 1`.
    ForeachInit(u16),
    ForeachNext { base: u16, key: u16, value: u16, exit: u32 },
    PushTrap(u32),
    PopTrap(u16),
    Throw,
    /// [parent, table] -> [table]
    Delegate,
}

/// Where a closure's captured value comes from when it is created.
#[derive(Clone, Copy, Debug)]
pub enum OuterSource {
    Local(u16),
    Outer(u16),
}

pub struct Proto {
    pub name: SqStr,
    pub source: SqStr,
    pub params: Vec<SqStr>,
    pub ndefaults: usize,
    pub varargs: bool,
    pub nslots: usize,
    pub code: Vec<Op>,
    pub lines: Vec<u32>,
    pub consts: Vec<Value>,
    pub protos: Vec<Arc<Proto>>,
    pub outers: Vec<OuterSource>,
    pub generator: bool,
}

/// Constants visible to later compilations (`const`, `enum`).
pub type ConstTable = HashMap<Vec<u8>, Value>;

/// Hook for building enum tables on the heap.
pub trait ConstBuilder {
    fn make_table(&mut self, entries: Vec<(Value, Value)>) -> Value;
}

struct Scope {
    locals: Vec<(Name, u16)>,
}

struct Loop {
    breaks: Vec<usize>,
    continues: Vec<usize>,
    /// Traps active when the loop started.
    traps: usize,
    /// Switches accept break but not continue.
    is_switch: bool,
}

struct FuncState {
    name: SqStr,
    params: Vec<SqStr>,
    ndefaults: usize,
    varargs: bool,
    code: Vec<Op>,
    lines: Vec<u32>,
    consts: Vec<Value>,
    const_index: HashMap<Vec<u8>, u32>,
    protos: Vec<Arc<Proto>>,
    outers: Vec<(Name, OuterSource)>,
    scopes: Vec<Scope>,
    next_slot: u16,
    max_slot: u16,
    loops: Vec<Loop>,
    traps: usize,
    generator: bool,
    line: u32,
}

impl FuncState {
    fn new(name: SqStr) -> Self {
        Self {
            name,
            params: Vec::new(),
            ndefaults: 0,
            varargs: false,
            code: Vec::new(),
            lines: Vec::new(),
            consts: Vec::new(),
            const_index: HashMap::new(),
            protos: Vec::new(),
            outers: Vec::new(),
            scopes: vec![Scope { locals: Vec::new() }],
            // Slot 0 is `this`.
            next_slot: 1,
            max_slot: 1,
            loops: Vec::new(),
            traps: 0,
            generator: false,
            line: 0,
        }
    }

    fn emit(&mut self, op: Op) -> usize {
        self.code.push(op);
        self.lines.push(self.line);
        self.code.len() - 1
    }

    fn here(&self) -> u32 {
        self.code.len() as u32
    }

    fn patch(&mut self, at: usize, target: u32) {
        self.code[at] = match self.code[at] {
            Op::Jump(_) => Op::Jump(target),
            Op::JumpIfFalse(_) => Op::JumpIfFalse(target),
            Op::JumpIfTrue(_) => Op::JumpIfTrue(target),
            Op::AndJump(_) => Op::AndJump(target),
            Op::OrJump(_) => Op::OrJump(target),
            Op::PushTrap(_) => Op::PushTrap(target),
            Op::ForeachNext { base, key, value, .. } => Op::ForeachNext { base, key, value, exit: target },
            other => other,
        };
    }

    fn string_const(&mut self, s: &[u8]) -> u32 {
        if let Some(&i) = self.const_index.get(s) {
            return i;
        }
        self.consts.push(Value::Str(SqStr::new(s)));
        let i = (self.consts.len() - 1) as u32;
        self.const_index.insert(s.to_vec(), i);
        i
    }

    fn value_const(&mut self, v: Value) -> u32 {
        self.consts.push(v);
        (self.consts.len() - 1) as u32
    }

    fn declare_local(&mut self, name: &[u8]) -> u16 {
        let slot = self.next_slot;
        self.next_slot += 1;
        self.max_slot = self.max_slot.max(self.next_slot);
        self.scopes.last_mut().expect("scope").locals.push((name.to_vec(), slot));
        slot
    }

    fn find_local(&self, name: &[u8]) -> Option<u16> {
        for scope in self.scopes.iter().rev() {
            if let Some((_, slot)) = scope.locals.iter().rev().find(|(n, _)| n == name) {
                return Some(*slot);
            }
        }
        None
    }

    fn push_scope(&mut self) {
        self.scopes.push(Scope { locals: Vec::new() });
    }

    fn pop_scope(&mut self) {
        let scope = self.scopes.pop().expect("scope");
        self.next_slot -= scope.locals.len() as u16;
    }
}

pub struct Compiler<'a> {
    funcs: Vec<FuncState>,
    source: SqStr,
    consts: &'a mut ConstTable,
    builder: &'a mut dyn ConstBuilder,
}

enum Resolved {
    Local(u16),
    Outer(u16),
    Const(Value),
    Env,
}

pub fn compile(
    stmts: &[Stmt],
    source_name: &str,
    consts: &mut ConstTable,
    builder: &mut dyn ConstBuilder,
) -> Result<Arc<Proto>> {
    let source = SqStr::from(source_name);
    let mut c = Compiler { funcs: vec![FuncState::new(SqStr::from("main"))], source, consts, builder };
    // Scripts accept any arguments.
    c.funcs[0].varargs = true;
    for s in stmts {
        c.statement(s)?;
    }
    c.f().emit(Op::ReturnNull);
    let f = c.funcs.pop().expect("main");
    Ok(Arc::new(c.finish(f)))
}

impl Compiler<'_> {
    fn f(&mut self) -> &mut FuncState {
        self.funcs.last_mut().expect("function")
    }

    fn err(&self, msg: impl Into<String>, line: u32) -> Error {
        Error::compile(msg, &self.source.to_string_lossy(), line)
    }

    fn finish(&self, f: FuncState) -> Proto {
        Proto {
            name: f.name,
            source: self.source.clone(),
            params: f.params,
            ndefaults: f.ndefaults,
            varargs: f.varargs,
            nslots: f.max_slot as usize,
            code: f.code,
            lines: f.lines,
            consts: f.consts,
            protos: f.protos,
            outers: f.outers.into_iter().map(|(_, s)| s).collect(),
            generator: f.generator,
        }
    }

    fn resolve(&mut self, name: &[u8]) -> Resolved {
        let depth = self.funcs.len() - 1;
        if let Some(slot) = self.funcs[depth].find_local(name) {
            return Resolved::Local(slot);
        }
        if let Some(i) = self.resolve_outer(depth, name) {
            return Resolved::Outer(i);
        }
        if let Some(v) = self.consts.get(name) {
            return Resolved::Const(v.clone());
        }
        Resolved::Env
    }

    /// Finds `name` in enclosing functions, adding capture entries to every
    /// function in between.
    fn resolve_outer(&mut self, depth: usize, name: &[u8]) -> Option<u16> {
        if let Some(i) = self.funcs[depth].outers.iter().position(|(n, _)| n == name) {
            return Some(i as u16);
        }
        if depth == 0 {
            return None;
        }
        let source = if let Some(slot) = self.funcs[depth - 1].find_local(name) {
            OuterSource::Local(slot)
        } else {
            OuterSource::Outer(self.resolve_outer(depth - 1, name)?)
        };
        self.funcs[depth].outers.push((name.to_vec(), source));
        Some((self.funcs[depth].outers.len() - 1) as u16)
    }

    // ------------------------------------------------------------ statements

    fn block(&mut self, stmts: &[Stmt]) -> Result<()> {
        self.f().push_scope();
        for s in stmts {
            self.statement(s)?;
        }
        self.f().pop_scope();
        Ok(())
    }

    /// A statement in its own scope (loop bodies, branches).
    fn scoped(&mut self, s: &Stmt) -> Result<()> {
        self.f().push_scope();
        self.statement(s)?;
        self.f().pop_scope();
        Ok(())
    }

    fn statement(&mut self, s: &Stmt) -> Result<()> {
        self.f().line = s.line;
        match &s.kind {
            StmtKind::Empty => {}
            StmtKind::Expr(e) => {
                self.expr(e)?;
                self.f().emit(Op::Pop);
            }
            StmtKind::Local(decls) => {
                for (name, init) in decls {
                    match init {
                        Some(e) => self.expr(e)?,
                        None => {
                            self.f().emit(Op::Null);
                        }
                    }
                    let slot = self.f().declare_local(name);
                    self.f().emit(Op::SetLocal(slot));
                    self.f().emit(Op::Pop);
                }
            }
            StmtKind::LocalFunction(name, def) => {
                let slot = self.f().declare_local(name);
                self.function(def)?;
                self.f().emit(Op::SetLocal(slot));
                self.f().emit(Op::Pop);
            }
            StmtKind::Function(path, def) => {
                if path.len() == 1 {
                    self.function(def)?;
                    let k = self.f().string_const(&path[0]);
                    self.f().emit(Op::NewSlotEnv(k));
                } else {
                    self.load_name(&path[0], s.line)?;
                    for mid in &path[1..path.len() - 1] {
                        let k = self.f().string_const(mid);
                        self.f().emit(Op::GetK(k));
                    }
                    let k = self.f().string_const(path.last().expect("path"));
                    self.f().emit(Op::Const(k));
                    self.function(def)?;
                    self.f().emit(Op::NewSlot);
                }
                self.f().emit(Op::Pop);
            }
            StmtKind::Class(target, def) => {
                match &target.kind {
                    ExprKind::Ident(name) => {
                        self.class(def)?;
                        let k = self.f().string_const(name);
                        self.f().emit(Op::NewSlotEnv(k));
                    }
                    ExprKind::Root(name) => {
                        self.f().emit(Op::Root);
                        let k = self.f().string_const(name);
                        self.f().emit(Op::Const(k));
                        self.class(def)?;
                        self.f().emit(Op::NewSlot);
                    }
                    ExprKind::Field(obj, name) => {
                        self.expr(obj)?;
                        let k = self.f().string_const(name);
                        self.f().emit(Op::Const(k));
                        self.class(def)?;
                        self.f().emit(Op::NewSlot);
                    }
                    _ => return Err(self.err("invalid class name", s.line)),
                }
                self.f().emit(Op::Pop);
            }
            StmtKind::If(cond, then, otherwise) => {
                self.expr(cond)?;
                let jf = self.f().emit(Op::JumpIfFalse(0));
                self.scoped(then)?;
                match otherwise {
                    Some(o) => {
                        let jend = self.f().emit(Op::Jump(0));
                        let here = self.f().here();
                        self.f().patch(jf, here);
                        self.scoped(o)?;
                        let here = self.f().here();
                        self.f().patch(jend, here);
                    }
                    None => {
                        let here = self.f().here();
                        self.f().patch(jf, here);
                    }
                }
            }
            StmtKind::While(cond, body) => {
                let start = self.f().here();
                self.expr(cond)?;
                let jf = self.f().emit(Op::JumpIfFalse(0));
                self.begin_loop(false);
                self.scoped(body)?;
                self.f().emit(Op::Jump(start));
                let end = self.f().here();
                self.f().patch(jf, end);
                self.end_loop(start, end);
            }
            StmtKind::DoWhile(body, cond) => {
                let start = self.f().here();
                self.begin_loop(false);
                self.scoped(body)?;
                let cont = self.f().here();
                self.expr(cond)?;
                self.f().emit(Op::JumpIfTrue(start));
                let end = self.f().here();
                self.end_loop(cont, end);
            }
            StmtKind::For(init, cond, step, body) => {
                self.f().push_scope();
                if let Some(i) = init {
                    self.statement(i)?;
                }
                let start = self.f().here();
                let jf = match cond {
                    Some(c) => {
                        self.expr(c)?;
                        Some(self.f().emit(Op::JumpIfFalse(0)))
                    }
                    None => None,
                };
                self.begin_loop(false);
                self.scoped(body)?;
                let cont = self.f().here();
                if let Some(st) = step {
                    self.expr(st)?;
                    self.f().emit(Op::Pop);
                }
                self.f().emit(Op::Jump(start));
                let end = self.f().here();
                if let Some(jf) = jf {
                    self.f().patch(jf, end);
                }
                self.end_loop(cont, end);
                self.f().pop_scope();
            }
            StmtKind::Foreach { key, value, iter, body } => {
                self.f().push_scope();
                self.expr(iter)?;
                let base = self.f().declare_local(b"@container");
                self.f().declare_local(b"@iterator");
                self.f().emit(Op::SetLocal(base));
                self.f().emit(Op::Pop);
                self.f().emit(Op::ForeachInit(base));
                let key_slot = match key {
                    Some(k) => self.f().declare_local(k),
                    None => u16::MAX,
                };
                let value_slot = self.f().declare_local(value);
                let start = self.f().here();
                let next = self.f().emit(Op::ForeachNext { base, key: key_slot, value: value_slot, exit: 0 });
                self.begin_loop(false);
                self.scoped(body)?;
                self.f().emit(Op::Jump(start));
                let end = self.f().here();
                self.f().patch(next, end);
                self.end_loop(start, end);
                self.f().pop_scope();
            }
            StmtKind::Switch(value, cases, default) => {
                self.f().push_scope();
                self.expr(value)?;
                let tmp = self.f().declare_local(b"@switch");
                self.f().emit(Op::SetLocal(tmp));
                self.f().emit(Op::Pop);
                let mut jumps = Vec::new();
                for (test, _) in cases {
                    self.f().emit(Op::GetLocal(tmp));
                    self.expr(test)?;
                    self.f().emit(Op::Binary(BinOp::Eq));
                    jumps.push(self.f().emit(Op::JumpIfTrue(0)));
                }
                let jdefault = self.f().emit(Op::Jump(0));
                self.begin_loop(true);
                for ((_, body), j) in cases.iter().zip(jumps) {
                    let here = self.f().here();
                    self.f().patch(j, here);
                    self.block(body)?;
                }
                let here = self.f().here();
                self.f().patch(jdefault, here);
                if let Some(d) = default {
                    self.block(d)?;
                }
                let end = self.f().here();
                self.end_loop(end, end);
                self.f().pop_scope();
            }
            StmtKind::Break => {
                let f = self.f();
                let Some(lp) = f.loops.last() else {
                    return Err(self.err("'break' has to be in a loop block", s.line));
                };
                let pops = f.traps - lp.traps;
                if pops > 0 {
                    f.emit(Op::PopTrap(pops as u16));
                }
                let j = f.emit(Op::Jump(0));
                f.loops.last_mut().expect("loop").breaks.push(j);
            }
            StmtKind::Continue => {
                let f = self.f();
                let Some(idx) = f.loops.iter().rposition(|l| !l.is_switch) else {
                    return Err(self.err("'continue' has to be in a loop block", s.line));
                };
                let pops = f.traps - f.loops[idx].traps;
                if pops > 0 {
                    f.emit(Op::PopTrap(pops as u16));
                }
                let j = f.emit(Op::Jump(0));
                f.loops[idx].continues.push(j);
            }
            StmtKind::Return(value) => match value {
                Some(v) => {
                    self.expr(v)?;
                    self.f().emit(Op::Return);
                }
                None => {
                    self.f().emit(Op::ReturnNull);
                }
            },
            StmtKind::Yield(value) => {
                self.f().generator = true;
                match value {
                    Some(v) => {
                        self.expr(v)?;
                        self.f().emit(Op::Yield);
                    }
                    None => {
                        self.f().emit(Op::YieldNull);
                    }
                }
            }
            StmtKind::Block(stmts) => self.block(stmts)?,
            StmtKind::Try(body, name, handler) => {
                let trap = self.f().emit(Op::PushTrap(0));
                self.f().traps += 1;
                self.scoped(body)?;
                self.f().traps -= 1;
                self.f().emit(Op::PopTrap(1));
                let jend = self.f().emit(Op::Jump(0));
                let here = self.f().here();
                self.f().patch(trap, here);
                self.f().push_scope();
                let slot = self.f().declare_local(name);
                self.f().emit(Op::SetLocal(slot));
                self.f().emit(Op::Pop);
                self.statement(handler)?;
                self.f().pop_scope();
                let here = self.f().here();
                self.f().patch(jend, here);
            }
            StmtKind::Throw(v) => {
                self.expr(v)?;
                self.f().emit(Op::Throw);
            }
            StmtKind::Const(name, value) => {
                let v = literal(value).ok_or_else(|| self.err("scalar expected : integer,float or string", s.line))?;
                self.consts.insert(name.clone(), v);
            }
            StmtKind::Enum(name, items) => {
                let mut next = 0i32;
                let mut entries = Vec::new();
                for (item, value) in items {
                    let v = match value {
                        Some(e) => literal(e).ok_or_else(|| self.err("scalar expected : integer,float or string", s.line))?,
                        None => Value::Int(next),
                    };
                    if let Value::Int(i) = v {
                        next = i.wrapping_add(1);
                    }
                    entries.push((Value::Str(SqStr::new(item)), v));
                }
                let table = self.builder.make_table(entries);
                self.consts.insert(name.clone(), table);
            }
        }
        Ok(())
    }

    fn begin_loop(&mut self, is_switch: bool) {
        let traps = self.f().traps;
        self.f().loops.push(Loop { breaks: Vec::new(), continues: Vec::new(), traps, is_switch });
    }

    fn end_loop(&mut self, continue_target: u32, end: u32) {
        let lp = self.f().loops.pop().expect("loop");
        for b in lp.breaks {
            self.f().patch(b, end);
        }
        for c in lp.continues {
            self.f().patch(c, continue_target);
        }
    }

    // ----------------------------------------------------------- expressions

    fn load_name(&mut self, name: &[u8], line: u32) -> Result<()> {
        let _ = line;
        match self.resolve(name) {
            Resolved::Local(slot) => {
                self.f().emit(Op::GetLocal(slot));
            }
            Resolved::Outer(i) => {
                self.f().emit(Op::GetOuter(i));
            }
            Resolved::Const(v) => self.push_value(v),
            Resolved::Env => {
                let k = self.f().string_const(name);
                self.f().emit(Op::GetEnv(k));
            }
        }
        Ok(())
    }

    fn push_value(&mut self, v: Value) {
        let op = match v {
            Value::Null => Op::Null,
            Value::Bool(true) => Op::True,
            Value::Bool(false) => Op::False,
            Value::Int(i) => Op::Int(i),
            Value::Float(x) => Op::Float(x),
            other => Op::Const(self.f().value_const(other)),
        };
        self.f().emit(op);
    }

    fn expr(&mut self, e: &Expr) -> Result<()> {
        self.f().line = e.line;
        match &e.kind {
            ExprKind::Null => {
                self.f().emit(Op::Null);
            }
            ExprKind::Bool(b) => {
                self.f().emit(if *b { Op::True } else { Op::False });
            }
            ExprKind::Int(i) => {
                self.f().emit(Op::Int(*i));
            }
            ExprKind::Float(x) => {
                self.f().emit(Op::Float(*x));
            }
            ExprKind::Str(s) => {
                let k = self.f().string_const(s);
                self.f().emit(Op::Const(k));
            }
            ExprKind::Ident(name) => self.load_name(name, e.line)?,
            ExprKind::This => {
                self.f().emit(Op::This);
            }
            ExprKind::Parent => {
                self.f().emit(Op::Parent);
            }
            ExprKind::ParentOf(obj) => {
                self.expr(obj)?;
                self.f().emit(Op::ParentOf);
            }
            ExprKind::Base => {
                self.f().emit(Op::Base);
            }
            ExprKind::Root(name) => {
                self.f().emit(Op::Root);
                let k = self.f().string_const(name);
                self.f().emit(Op::GetK(k));
            }
            ExprKind::Vargc => {
                self.f().emit(Op::Vargc);
            }
            ExprKind::Vargv(idx) => {
                self.expr(idx)?;
                self.f().emit(Op::Vargv);
            }
            ExprKind::Field(obj, name) => {
                self.expr(obj)?;
                let k = self.f().string_const(name);
                self.f().emit(Op::GetK(k));
            }
            ExprKind::Index(obj, idx) => {
                self.expr(obj)?;
                self.expr(idx)?;
                self.f().emit(Op::Get);
            }
            ExprKind::Call(callee, args) => self.call(callee, args)?,
            ExprKind::Unary(op, inner) => {
                self.expr(inner)?;
                self.f().emit(Op::Unary(*op));
            }
            ExprKind::Binary(op, a, b) => {
                self.expr(a)?;
                self.expr(b)?;
                self.f().emit(Op::Binary(*op));
            }
            ExprKind::And(a, b) => {
                self.expr(a)?;
                let j = self.f().emit(Op::AndJump(0));
                self.expr(b)?;
                let here = self.f().here();
                self.f().patch(j, here);
            }
            ExprKind::Or(a, b) => {
                self.expr(a)?;
                let j = self.f().emit(Op::OrJump(0));
                self.expr(b)?;
                let here = self.f().here();
                self.f().patch(j, here);
            }
            ExprKind::Ternary(c, a, b) => {
                self.expr(c)?;
                let jf = self.f().emit(Op::JumpIfFalse(0));
                self.expr(a)?;
                let jend = self.f().emit(Op::Jump(0));
                let here = self.f().here();
                self.f().patch(jf, here);
                self.expr(b)?;
                let here = self.f().here();
                self.f().patch(jend, here);
            }
            ExprKind::Assign(op, target, value) => self.assign(*op, target, value, e.line)?,
            ExprKind::IncDec { target, delta, prefix } => {
                let (delta, post) = (*delta as i8, !*prefix);
                match &target.kind {
                    ExprKind::Ident(name) => match self.resolve(name) {
                        Resolved::Local(slot) => {
                            self.f().emit(Op::IncLocal { slot, delta, post });
                        }
                        Resolved::Outer(slot) => {
                            self.f().emit(Op::IncOuter { slot, delta, post });
                        }
                        Resolved::Const(_) => return Err(self.err("can't assign a constant", e.line)),
                        Resolved::Env => {
                            let name = self.f().string_const(name);
                            self.f().emit(Op::IncEnv { name, delta, post });
                        }
                    },
                    ExprKind::Field(obj, name) => {
                        self.expr(obj)?;
                        let k = self.f().string_const(name);
                        self.f().emit(Op::Const(k));
                        self.f().emit(Op::Inc { delta, post });
                    }
                    ExprKind::Index(obj, idx) => {
                        self.expr(obj)?;
                        self.expr(idx)?;
                        self.f().emit(Op::Inc { delta, post });
                    }
                    ExprKind::Root(name) => {
                        self.f().emit(Op::Root);
                        let k = self.f().string_const(name);
                        self.f().emit(Op::Const(k));
                        self.f().emit(Op::Inc { delta, post });
                    }
                    _ => return Err(self.err("can't '++' or '--' an expression", e.line)),
                }
            }
            ExprKind::Delete(target) => match &target.kind {
                ExprKind::Field(obj, name) => {
                    self.expr(obj)?;
                    let k = self.f().string_const(name);
                    self.f().emit(Op::Const(k));
                    self.f().emit(Op::Delete);
                }
                ExprKind::Index(obj, idx) => {
                    self.expr(obj)?;
                    self.expr(idx)?;
                    self.f().emit(Op::Delete);
                }
                ExprKind::Root(name) => {
                    self.f().emit(Op::Root);
                    let k = self.f().string_const(name);
                    self.f().emit(Op::Const(k));
                    self.f().emit(Op::Delete);
                }
                ExprKind::Ident(name) => {
                    if matches!(self.resolve(name), Resolved::Local(_) | Resolved::Outer(_)) {
                        return Err(self.err("cannot delete a local", e.line));
                    }
                    let k = self.f().string_const(name);
                    self.f().emit(Op::DeleteEnv(k));
                }
                _ => return Err(self.err("can't delete an expression", e.line)),
            },
            ExprKind::Table(slots) => {
                self.f().emit(Op::NewTable(slots.len() as u32));
                for (k, v) in slots {
                    self.expr(k)?;
                    self.expr(v)?;
                    self.f().emit(Op::TableSlot);
                }
            }
            ExprKind::Array(items) => {
                self.f().emit(Op::NewArray(items.len() as u32));
                for item in items {
                    self.expr(item)?;
                    self.f().emit(Op::ArrayAppend);
                }
            }
            ExprKind::Function(def) => self.function(def)?,
            ExprKind::Class(def) => self.class(def)?,
            ExprKind::Delegate(parent, table) => {
                self.expr(parent)?;
                self.expr(table)?;
                self.f().emit(Op::Delegate);
            }
            ExprKind::Comma(list) => {
                for (i, item) in list.iter().enumerate() {
                    self.expr(item)?;
                    if i + 1 < list.len() {
                        self.f().emit(Op::Pop);
                    }
                }
            }
        }
        Ok(())
    }

    fn call(&mut self, callee: &Expr, args: &[Expr]) -> Result<()> {
        match &callee.kind {
            // Member calls pass the object as `this`.
            ExprKind::Field(obj, name) => {
                self.expr(obj)?;
                self.f().emit(Op::Dup);
                let k = self.f().string_const(name);
                self.f().emit(Op::GetK(k));
                self.f().emit(Op::Swap);
            }
            ExprKind::Index(obj, idx) => {
                self.expr(obj)?;
                self.f().emit(Op::Dup);
                self.expr(idx)?;
                self.f().emit(Op::Get);
                self.f().emit(Op::Swap);
            }
            ExprKind::Root(name) => {
                self.f().emit(Op::Root);
                self.f().emit(Op::Dup);
                let k = self.f().string_const(name);
                self.f().emit(Op::GetK(k));
                self.f().emit(Op::Swap);
            }
            // Plain calls pass the caller's `this`.
            _ => {
                self.expr(callee)?;
                self.f().emit(Op::This);
            }
        }
        for a in args {
            self.expr(a)?;
        }
        self.f().line = callee.line;
        self.f().emit(Op::Call(args.len() as u16));
        Ok(())
    }

    fn arith(op: AssignOp) -> Option<BinOp> {
        match op {
            AssignOp::Add => Some(BinOp::Add),
            AssignOp::Sub => Some(BinOp::Sub),
            AssignOp::Mul => Some(BinOp::Mul),
            AssignOp::Div => Some(BinOp::Div),
            AssignOp::Mod => Some(BinOp::Mod),
            _ => None,
        }
    }

    fn assign(&mut self, op: AssignOp, target: &Expr, value: &Expr, line: u32) -> Result<()> {
        let compound = Self::arith(op);
        match &target.kind {
            ExprKind::Ident(name) => match self.resolve(name) {
                Resolved::Local(slot) | Resolved::Outer(slot) if op == AssignOp::NewSlot => {
                    let _ = slot;
                    return Err(self.err("can't 'create' a local slot", line));
                }
                Resolved::Local(slot) => {
                    if let Some(b) = compound {
                        self.f().emit(Op::GetLocal(slot));
                        self.expr(value)?;
                        self.f().emit(Op::Binary(b));
                    } else {
                        self.expr(value)?;
                    }
                    self.f().emit(Op::SetLocal(slot));
                }
                Resolved::Outer(slot) => {
                    if let Some(b) = compound {
                        self.f().emit(Op::GetOuter(slot));
                        self.expr(value)?;
                        self.f().emit(Op::Binary(b));
                    } else {
                        self.expr(value)?;
                    }
                    self.f().emit(Op::SetOuter(slot));
                }
                Resolved::Const(_) => return Err(self.err("can't assign a constant", line)),
                Resolved::Env => {
                    let k = self.f().string_const(name);
                    if let Some(b) = compound {
                        self.f().emit(Op::GetEnv(k));
                        self.expr(value)?;
                        self.f().emit(Op::Binary(b));
                    } else {
                        self.expr(value)?;
                    }
                    self.f().emit(if op == AssignOp::NewSlot { Op::NewSlotEnv(k) } else { Op::SetEnv(k) });
                }
            },
            ExprKind::Field(..) | ExprKind::Index(..) | ExprKind::Root(_) => {
                match &target.kind {
                    ExprKind::Field(obj, name) => {
                        self.expr(obj)?;
                        let k = self.f().string_const(name);
                        self.f().emit(Op::Const(k));
                    }
                    ExprKind::Index(obj, idx) => {
                        self.expr(obj)?;
                        self.expr(idx)?;
                    }
                    ExprKind::Root(name) => {
                        self.f().emit(Op::Root);
                        let k = self.f().string_const(name);
                        self.f().emit(Op::Const(k));
                    }
                    _ => unreachable!(),
                }
                if let Some(b) = compound {
                    self.f().emit(Op::Dup2);
                    self.f().emit(Op::Get);
                    self.expr(value)?;
                    self.f().emit(Op::Binary(b));
                } else {
                    self.expr(value)?;
                }
                self.f().line = line;
                self.f().emit(if op == AssignOp::NewSlot { Op::NewSlot } else { Op::Set });
            }
            ExprKind::Vargv(_) => return Err(self.err("can't assign to vargv", line)),
            _ => return Err(self.err("can't assign expression", line)),
        }
        Ok(())
    }

    fn function(&mut self, def: &FuncDef) -> Result<()> {
        // Defaults are evaluated in the enclosing function at creation.
        let mut ndefaults = 0u16;
        for (_, d) in &def.params {
            if let Some(d) = d {
                self.expr(d)?;
                ndefaults += 1;
            }
        }
        let name = def.name.as_deref().map(SqStr::new).unwrap_or_else(|| SqStr::from("unnamed"));
        let mut fs = FuncState::new(name);
        fs.line = def.line;
        fs.varargs = def.varargs;
        fs.ndefaults = ndefaults as usize;
        self.funcs.push(fs);
        for (p, _) in &def.params {
            self.f().params.push(SqStr::new(p));
            self.f().declare_local(p);
        }
        for s in &def.body {
            self.statement(s)?;
        }
        self.f().emit(Op::ReturnNull);
        let fs = self.funcs.pop().expect("function");
        let proto = Arc::new(self.finish(fs));
        let f = self.f();
        f.protos.push(proto);
        let idx = (f.protos.len() - 1) as u32;
        f.line = def.line;
        f.emit(Op::Closure { proto: idx, defaults: ndefaults });
        Ok(())
    }

    fn class(&mut self, def: &ClassDef) -> Result<()> {
        if let Some(b) = &def.base {
            self.expr(b)?;
        }
        if let Some(a) = &def.attrs {
            self.expr(a)?;
        }
        self.f().emit(Op::NewClass { base: def.base.is_some(), attrs: def.attrs.is_some() });
        for m in &def.members {
            self.expr(&m.key)?;
            self.expr(&m.value)?;
            if let Some(a) = &m.attrs {
                self.expr(a)?;
            }
            self.f().emit(Op::ClassMember { is_static: m.is_static, attrs: m.attrs.is_some() });
        }
        Ok(())
    }
}

fn literal(e: &Expr) -> Option<Value> {
    Some(match &e.kind {
        ExprKind::Int(i) => Value::Int(*i),
        ExprKind::Float(f) => Value::Float(*f),
        ExprKind::Str(s) => Value::Str(SqStr::new(s)),
        ExprKind::Bool(b) => Value::Bool(*b),
        _ => return None,
    })
}
