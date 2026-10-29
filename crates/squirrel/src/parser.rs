//! Recursive descent parser for Squirrel 2.2.
//!
//! Operator levels follow the original compiler: `||`, then `&&` together
//! with `in` and `instanceof`, then `|`, `^`, `&`, then all comparisons
//! (equality and ordering share a level), shifts, `+ -`, `* / %`, unary
//! operators and postfix member access, indexing, calls and `++`/`--`.
//! Statements end at `;`, a line break, `}` or the end of the input.

use std::rc::Rc;

use crate::ast::*;
use crate::error::{Error, Result};
use crate::lexer::{Tok, Token, tokenize};

pub fn parse(src: &[u8], source_name: &str) -> Result<Vec<Stmt>> {
    let tokens = tokenize(src, source_name)?;
    let mut p = Parser { tokens, pos: 0, name: source_name.to_owned() };
    let mut out = Vec::new();
    while !p.at_eof() {
        out.push(p.statement()?);
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    name: String,
}

impl Parser {
    fn tok(&self) -> &Tok {
        &self.tokens[self.pos].tok
    }

    fn line(&self) -> u32 {
        self.tokens[self.pos].line
    }

    fn at_eof(&self) -> bool {
        *self.tok() == Tok::Eof
    }

    fn advance(&mut self) -> Tok {
        let t = self.tokens[self.pos].tok.clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn is_op(&self, op: &str) -> bool {
        matches!(self.tok(), Tok::Op(o) if *o == op)
    }

    fn eat_op(&mut self, op: &str) -> bool {
        if self.is_op(op) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn err(&self, msg: impl Into<String>) -> Error {
        Error::compile(msg, &self.name, self.line())
    }

    fn expect_op(&mut self, op: &str) -> Result<()> {
        if self.eat_op(op) { Ok(()) } else { Err(self.err(format!("expected '{op}'"))) }
    }

    fn ident(&mut self) -> Result<Name> {
        match self.tok().clone() {
            Tok::Ident(n) => {
                self.advance();
                Ok(n)
            }
            // `constructor` is an ordinary name outside class bodies.
            Tok::Constructor => {
                self.advance();
                Ok(b"constructor".to_vec())
            }
            _ => Err(self.err("expected 'IDENTIFIER'")),
        }
    }

    fn end_of_statement(&self) -> bool {
        self.tokens[self.pos].newline_before || self.at_eof() || self.is_op("}") || self.is_op(";")
    }

    fn optional_semicolon(&mut self) -> Result<()> {
        if self.eat_op(";") {
            return Ok(());
        }
        if !self.end_of_statement() {
            return Err(self.err("end of statement expected (; or lf)"));
        }
        Ok(())
    }

    // ------------------------------------------------------------ statements

    fn statement(&mut self) -> Result<Stmt> {
        let line = self.line();
        let kind = match self.tok().clone() {
            Tok::Op(";") => {
                self.advance();
                StmtKind::Empty
            }
            Tok::If => {
                self.advance();
                self.expect_op("(")?;
                let cond = self.comma_expr()?;
                self.expect_op(")")?;
                let then = Box::new(self.statement()?);
                // `else` may follow a `;` ending the then-branch.
                let save = self.pos;
                let _ = self.eat_op(";");
                let otherwise = if *self.tok() == Tok::Else {
                    self.advance();
                    Some(Box::new(self.statement()?))
                } else {
                    self.pos = save;
                    None
                };
                StmtKind::If(cond, then, otherwise)
            }
            Tok::While => {
                self.advance();
                self.expect_op("(")?;
                let cond = self.comma_expr()?;
                self.expect_op(")")?;
                StmtKind::While(cond, Box::new(self.statement()?))
            }
            Tok::Do => {
                self.advance();
                let body = Box::new(self.statement()?);
                if *self.tok() != Tok::While {
                    return Err(self.err("expected 'while'"));
                }
                self.advance();
                self.expect_op("(")?;
                let cond = self.comma_expr()?;
                self.expect_op(")")?;
                StmtKind::DoWhile(body, cond)
            }
            Tok::For => self.for_statement()?,
            Tok::Foreach => self.foreach_statement()?,
            Tok::Switch => self.switch_statement()?,
            Tok::Local => self.local_statement()?,
            Tok::Return | Tok::Yield => {
                let is_yield = *self.tok() == Tok::Yield;
                self.advance();
                let value = if self.end_of_statement() { None } else { Some(self.comma_expr()?) };
                self.optional_semicolon()?;
                if is_yield { StmtKind::Yield(value) } else { StmtKind::Return(value) }
            }
            Tok::Break => {
                self.advance();
                self.optional_semicolon()?;
                StmtKind::Break
            }
            Tok::Continue => {
                self.advance();
                self.optional_semicolon()?;
                StmtKind::Continue
            }
            Tok::Function => {
                self.advance();
                let mut path = vec![self.ident()?];
                while self.eat_op("::") {
                    path.push(self.ident()?);
                }
                let name = path.last().cloned();
                let def = self.function_body(name, line)?;
                StmtKind::Function(path, Rc::new(def))
            }
            Tok::Class => {
                self.advance();
                let target = self.class_target()?;
                let def = self.class_body()?;
                StmtKind::Class(target, Rc::new(def))
            }
            Tok::Op("{") => {
                self.advance();
                let mut body = Vec::new();
                while !self.is_op("}") {
                    if self.at_eof() {
                        return Err(self.err("expected '}'"));
                    }
                    body.push(self.statement()?);
                }
                self.advance();
                StmtKind::Block(body)
            }
            Tok::Try => {
                self.advance();
                let body = Box::new(self.statement()?);
                if *self.tok() != Tok::Catch {
                    return Err(self.err("expected 'catch'"));
                }
                self.advance();
                self.expect_op("(")?;
                let name = self.ident()?;
                self.expect_op(")")?;
                StmtKind::Try(body, name, Box::new(self.statement()?))
            }
            Tok::Throw => {
                self.advance();
                let value = self.comma_expr()?;
                self.optional_semicolon()?;
                StmtKind::Throw(value)
            }
            Tok::Const => {
                self.advance();
                let name = self.ident()?;
                self.expect_op("=")?;
                let value = self.const_value()?;
                self.optional_semicolon()?;
                StmtKind::Const(name, value)
            }
            Tok::Enum => {
                self.advance();
                let name = self.ident()?;
                self.expect_op("{")?;
                let mut items = Vec::new();
                while !self.is_op("}") {
                    let item = self.ident()?;
                    let value = if self.eat_op("=") { Some(self.const_value()?) } else { None };
                    items.push((item, value));
                    let _ = self.eat_op(",");
                }
                self.advance();
                let _ = self.eat_op(";");
                StmtKind::Enum(name, items)
            }
            _ => {
                let e = self.comma_expr()?;
                self.optional_semicolon()?;
                StmtKind::Expr(e)
            }
        };
        Ok(Stmt { kind, line })
    }

    /// Literal (optionally negated) for `const` and `enum`.
    fn const_value(&mut self) -> Result<Expr> {
        let line = self.line();
        let neg = self.eat_op("-");
        let kind = match self.advance() {
            Tok::Int(i) => ExprKind::Int(if neg { i.wrapping_neg() } else { i }),
            Tok::Float(f) => ExprKind::Float(if neg { -f } else { f }),
            Tok::Str(s) if !neg => ExprKind::Str(s),
            Tok::True if !neg => ExprKind::Bool(true),
            Tok::False if !neg => ExprKind::Bool(false),
            _ => return Err(self.err("scalar expected : integer,float or string")),
        };
        Ok(Expr::new(kind, line))
    }

    fn for_statement(&mut self) -> Result<StmtKind> {
        self.advance();
        self.expect_op("(")?;
        let init = if self.is_op(";") {
            None
        } else if *self.tok() == Tok::Local {
            let line = self.line();
            let kind = self.local_declarations()?;
            Some(Box::new(Stmt { kind, line }))
        } else {
            let line = self.line();
            Some(Box::new(Stmt { kind: StmtKind::Expr(self.comma_expr()?), line }))
        };
        self.expect_op(";")?;
        let cond = if self.is_op(";") { None } else { Some(self.comma_expr()?) };
        self.expect_op(";")?;
        let step = if self.is_op(")") { None } else { Some(self.comma_expr()?) };
        self.expect_op(")")?;
        let body = Box::new(self.statement()?);
        Ok(StmtKind::For(init, cond, step, body))
    }

    fn foreach_statement(&mut self) -> Result<StmtKind> {
        self.advance();
        self.expect_op("(")?;
        let first = self.ident()?;
        let (key, value) = if self.eat_op(",") { (Some(first), self.ident()?) } else { (None, first) };
        if *self.tok() != Tok::In {
            return Err(self.err("expected 'in'"));
        }
        self.advance();
        let iter = self.comma_expr()?;
        self.expect_op(")")?;
        let body = Box::new(self.statement()?);
        Ok(StmtKind::Foreach { key, value, iter, body })
    }

    fn switch_statement(&mut self) -> Result<StmtKind> {
        self.advance();
        self.expect_op("(")?;
        let value = self.comma_expr()?;
        self.expect_op(")")?;
        self.expect_op("{")?;
        let mut cases = Vec::new();
        let mut default = None;
        loop {
            match self.tok() {
                Tok::Case => {
                    self.advance();
                    let test = self.expression()?;
                    self.expect_op(":")?;
                    cases.push((test, self.case_body()?));
                }
                Tok::Default => {
                    self.advance();
                    self.expect_op(":")?;
                    default = Some(self.case_body()?);
                }
                Tok::Op("}") => {
                    self.advance();
                    break;
                }
                _ => return Err(self.err("expected 'case', 'default' or '}'")),
            }
        }
        Ok(StmtKind::Switch(value, cases, default))
    }

    fn case_body(&mut self) -> Result<Vec<Stmt>> {
        let mut body = Vec::new();
        while !matches!(self.tok(), Tok::Case | Tok::Default | Tok::Op("}")) {
            if self.at_eof() {
                return Err(self.err("expected '}'"));
            }
            body.push(self.statement()?);
        }
        Ok(body)
    }

    fn local_statement(&mut self) -> Result<StmtKind> {
        let kind = self.local_declarations()?;
        if !matches!(kind, StmtKind::LocalFunction(..)) {
            self.optional_semicolon()?;
        }
        Ok(kind)
    }

    fn local_declarations(&mut self) -> Result<StmtKind> {
        self.advance(); // local
        if *self.tok() == Tok::Function {
            let line = self.line();
            self.advance();
            let name = self.ident()?;
            let def = self.function_body(Some(name.clone()), line)?;
            return Ok(StmtKind::LocalFunction(name, Rc::new(def)));
        }
        let mut decls = Vec::new();
        loop {
            let name = self.ident()?;
            let init = if self.eat_op("=") { Some(self.expression()?) } else { None };
            decls.push((name, init));
            if !self.eat_op(",") {
                break;
            }
        }
        Ok(StmtKind::Local(decls))
    }

    /// `(params) body` after `function [name]`.
    fn function_body(&mut self, name: Option<Name>, line: u32) -> Result<FuncDef> {
        self.expect_op("(")?;
        let mut params = Vec::new();
        let mut varargs = false;
        while !self.is_op(")") {
            if self.eat_op("...") {
                varargs = true;
                break;
            }
            let p = self.ident()?;
            let default = if self.eat_op("=") { Some(self.expression()?) } else { None };
            params.push((p, default));
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(")")?;
        let body_line = self.line();
        let body = match self.statement()? {
            Stmt { kind: StmtKind::Block(b), .. } => b,
            other => vec![Stmt { kind: other.kind, line: body_line }],
        };
        Ok(FuncDef { name, params, varargs, body, line })
    }

    /// `class` statement target: `name`, `a.b` or `::name`.
    fn class_target(&mut self) -> Result<Expr> {
        let line = self.line();
        let mut e = if self.eat_op("::") {
            Expr::new(ExprKind::Root(self.ident()?), line)
        } else {
            Expr::new(ExprKind::Ident(self.ident()?), line)
        };
        while self.eat_op(".") {
            let n = self.ident()?;
            e = Expr::new(ExprKind::Field(Box::new(e), n), line);
        }
        Ok(e)
    }

    fn attributes(&mut self) -> Result<Option<Expr>> {
        if !self.is_op("</") {
            return Ok(None);
        }
        let line = self.line();
        self.advance();
        let mut slots = Vec::new();
        while !self.is_op("/>") {
            let key = self.table_key()?;
            let value = if self.eat_op("=") { self.expression()? } else { Expr::new(ExprKind::Null, line) };
            slots.push((key, value));
            let _ = self.eat_op(",");
        }
        self.advance();
        Ok(Some(Expr::new(ExprKind::Table(slots), line)))
    }

    fn table_key(&mut self) -> Result<Expr> {
        let line = self.line();
        if self.eat_op("[") {
            let k = self.comma_expr()?;
            self.expect_op("]")?;
            return Ok(k);
        }
        Ok(Expr::new(ExprKind::Str(self.ident()?), line))
    }

    fn class_body(&mut self) -> Result<ClassDef> {
        let base = if *self.tok() == Tok::Extends {
            self.advance();
            Some(self.postfix_expr()?)
        } else {
            None
        };
        let attrs = self.attributes()?;
        self.expect_op("{")?;
        let mut members = Vec::new();
        while !self.is_op("}") {
            if self.at_eof() {
                return Err(self.err("expected '}'"));
            }
            let member_attrs = self.attributes()?;
            let is_static = if *self.tok() == Tok::Static {
                self.advance();
                true
            } else {
                false
            };
            let line = self.line();
            match self.tok().clone() {
                Tok::Function => {
                    self.advance();
                    let name = self.ident()?;
                    let def = self.function_body(Some(name.clone()), line)?;
                    members.push(ClassMember {
                        key: Expr::new(ExprKind::Str(name), line),
                        value: Expr::new(ExprKind::Function(Rc::new(def)), line),
                        is_static,
                        attrs: member_attrs,
                    });
                }
                Tok::Constructor => {
                    self.advance();
                    let def = self.function_body(Some(b"constructor".to_vec()), line)?;
                    members.push(ClassMember {
                        key: Expr::new(ExprKind::Str(b"constructor".to_vec()), line),
                        value: Expr::new(ExprKind::Function(Rc::new(def)), line),
                        is_static,
                        attrs: member_attrs,
                    });
                }
                _ => {
                    let key = self.table_key()?;
                    self.expect_op("=")?;
                    let value = self.expression()?;
                    members.push(ClassMember { key, value, is_static, attrs: member_attrs });
                }
            }
            let _ = self.eat_op(";");
        }
        self.advance();
        Ok(ClassDef { base, attrs, members })
    }

    // ----------------------------------------------------------- expressions

    /// Comma-separated expressions; the value is the last one.
    fn comma_expr(&mut self) -> Result<Expr> {
        let first = self.expression()?;
        if !self.is_op(",") {
            return Ok(first);
        }
        let line = first.line;
        let mut list = vec![first];
        while self.eat_op(",") {
            list.push(self.expression()?);
        }
        Ok(Expr::new(ExprKind::Comma(list), line))
    }

    fn expression(&mut self) -> Result<Expr> {
        let lhs = self.logical_or()?;
        let line = lhs.line;
        let op = match self.tok() {
            Tok::Op("=") => Some(AssignOp::Set),
            Tok::Op("<-") => Some(AssignOp::NewSlot),
            Tok::Op("+=") => Some(AssignOp::Add),
            Tok::Op("-=") => Some(AssignOp::Sub),
            Tok::Op("*=") => Some(AssignOp::Mul),
            Tok::Op("/=") => Some(AssignOp::Div),
            Tok::Op("%=") => Some(AssignOp::Mod),
            _ => None,
        };
        if let Some(op) = op {
            if !is_lvalue(&lhs) {
                return Err(self.err("can't assign expression"));
            }
            if op == AssignOp::NewSlot
                && !matches!(lhs.kind, ExprKind::Ident(_) | ExprKind::Field(..) | ExprKind::Index(..) | ExprKind::Root(_))
            {
                return Err(self.err("can't 'create' a local slot"));
            }
            self.advance();
            let rhs = self.expression()?;
            return Ok(Expr::new(ExprKind::Assign(op, Box::new(lhs), Box::new(rhs)), line));
        }
        if self.eat_op("?") {
            let a = self.expression()?;
            self.expect_op(":")?;
            let b = self.expression()?;
            return Ok(Expr::new(ExprKind::Ternary(Box::new(lhs), Box::new(a), Box::new(b)), line));
        }
        Ok(lhs)
    }

    fn logical_or(&mut self) -> Result<Expr> {
        let lhs = self.logical_and()?;
        if self.eat_op("||") {
            let line = lhs.line;
            let rhs = self.logical_or()?;
            return Ok(Expr::new(ExprKind::Or(Box::new(lhs), Box::new(rhs)), line));
        }
        Ok(lhs)
    }

    fn logical_and(&mut self) -> Result<Expr> {
        let mut lhs = self.bitwise_or()?;
        loop {
            let line = lhs.line;
            match self.tok() {
                Tok::Op("&&") => {
                    self.advance();
                    let rhs = self.logical_and()?;
                    lhs = Expr::new(ExprKind::And(Box::new(lhs), Box::new(rhs)), line);
                }
                Tok::In => {
                    self.advance();
                    let rhs = self.bitwise_or()?;
                    lhs = Expr::new(ExprKind::Binary(BinOp::In, Box::new(lhs), Box::new(rhs)), line);
                }
                Tok::Instanceof => {
                    self.advance();
                    let rhs = self.bitwise_or()?;
                    lhs = Expr::new(ExprKind::Binary(BinOp::InstanceOf, Box::new(lhs), Box::new(rhs)), line);
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn binary_level(&mut self, ops: &[(&str, BinOp)], next: fn(&mut Self) -> Result<Expr>) -> Result<Expr> {
        let mut lhs = next(self)?;
        'outer: loop {
            for (text, op) in ops {
                if self.is_op(text) {
                    self.advance();
                    let rhs = next(self)?;
                    let line = lhs.line;
                    lhs = Expr::new(ExprKind::Binary(*op, Box::new(lhs), Box::new(rhs)), line);
                    continue 'outer;
                }
            }
            return Ok(lhs);
        }
    }

    fn bitwise_or(&mut self) -> Result<Expr> {
        self.binary_level(&[("|", BinOp::BitOr)], Self::bitwise_xor)
    }

    fn bitwise_xor(&mut self) -> Result<Expr> {
        self.binary_level(&[("^", BinOp::BitXor)], Self::bitwise_and)
    }

    fn bitwise_and(&mut self) -> Result<Expr> {
        self.binary_level(&[("&", BinOp::BitAnd)], Self::comparison)
    }

    fn comparison(&mut self) -> Result<Expr> {
        self.binary_level(
            &[("==", BinOp::Eq), ("!=", BinOp::Ne), ("<=", BinOp::Le), (">=", BinOp::Ge), ("<", BinOp::Lt), (">", BinOp::Gt)],
            Self::shift,
        )
    }

    fn shift(&mut self) -> Result<Expr> {
        self.binary_level(&[(">>>", BinOp::UShr), ("<<", BinOp::Shl), (">>", BinOp::Shr)], Self::additive)
    }

    fn additive(&mut self) -> Result<Expr> {
        self.binary_level(&[("+", BinOp::Add), ("-", BinOp::Sub)], Self::multiplicative)
    }

    fn multiplicative(&mut self) -> Result<Expr> {
        self.binary_level(&[("*", BinOp::Mul), ("/", BinOp::Div), ("%", BinOp::Mod)], Self::unary)
    }

    fn unary(&mut self) -> Result<Expr> {
        let line = self.line();
        let op = match self.tok() {
            Tok::Op("-") => Some(UnOp::Neg),
            Tok::Op("!") => Some(UnOp::Not),
            Tok::Op("~") => Some(UnOp::BitNot),
            Tok::Typeof => Some(UnOp::TypeOf),
            Tok::Clone => Some(UnOp::Clone),
            Tok::Resume => Some(UnOp::Resume),
            _ => None,
        };
        if let Some(op) = op {
            self.advance();
            let inner = self.unary()?;
            // Fold negative literals.
            if op == UnOp::Neg {
                match inner.kind {
                    ExprKind::Int(i) => return Ok(Expr::new(ExprKind::Int(i.wrapping_neg()), line)),
                    ExprKind::Float(f) => return Ok(Expr::new(ExprKind::Float(-f), line)),
                    _ => {}
                }
            }
            return Ok(Expr::new(ExprKind::Unary(op, Box::new(inner)), line));
        }
        if self.is_op("++") || self.is_op("--") {
            let delta = if self.is_op("++") { 1 } else { -1 };
            self.advance();
            let target = self.unary()?;
            if !is_lvalue(&target) {
                return Err(self.err("can't '++' or '--' an expression"));
            }
            return Ok(Expr::new(ExprKind::IncDec { target: Box::new(target), delta, prefix: true }, line));
        }
        if *self.tok() == Tok::Delete {
            self.advance();
            let target = self.unary()?;
            if !matches!(target.kind, ExprKind::Field(..) | ExprKind::Index(..) | ExprKind::Ident(_) | ExprKind::Root(_)) {
                return Err(self.err("can't delete an expression"));
            }
            return Ok(Expr::new(ExprKind::Delete(Box::new(target)), line));
        }
        if *self.tok() == Tok::Delegate {
            self.advance();
            let parent = self.expression()?;
            self.expect_op(":")?;
            let table = self.expression()?;
            return Ok(Expr::new(ExprKind::Delegate(Box::new(parent), Box::new(table)), line));
        }
        self.postfix_expr()
    }

    fn postfix_expr(&mut self) -> Result<Expr> {
        let mut e = self.factor()?;
        loop {
            let line = self.line();
            if self.is_op(".") {
                self.advance();
                if *self.tok() == Tok::Parent {
                    self.advance();
                    e = Expr::new(ExprKind::ParentOf(Box::new(e)), line);
                    continue;
                }
                let name = self.ident()?;
                e = Expr::new(ExprKind::Field(Box::new(e), name), line);
            } else if self.is_op("[") {
                if self.tokens[self.pos].newline_before {
                    return Ok(e);
                }
                self.advance();
                let idx = self.comma_expr()?;
                self.expect_op("]")?;
                e = Expr::new(ExprKind::Index(Box::new(e), Box::new(idx)), line);
            } else if self.is_op("(") {
                self.advance();
                let mut args = Vec::new();
                while !self.is_op(")") {
                    args.push(self.expression()?);
                    if !self.eat_op(",") {
                        break;
                    }
                }
                self.expect_op(")")?;
                e = Expr::new(ExprKind::Call(Box::new(e), args), line);
            } else if (self.is_op("++") || self.is_op("--")) && !self.tokens[self.pos].newline_before {
                let delta = if self.is_op("++") { 1 } else { -1 };
                if !is_lvalue(&e) {
                    return Ok(e);
                }
                self.advance();
                e = Expr::new(ExprKind::IncDec { target: Box::new(e), delta, prefix: false }, line);
            } else {
                return Ok(e);
            }
        }
    }

    fn factor(&mut self) -> Result<Expr> {
        let line = self.line();
        let kind = match self.advance() {
            Tok::Null => ExprKind::Null,
            Tok::True => ExprKind::Bool(true),
            Tok::False => ExprKind::Bool(false),
            Tok::Int(i) => ExprKind::Int(i),
            Tok::Float(f) => ExprKind::Float(f),
            Tok::Str(s) => ExprKind::Str(s),
            Tok::Ident(n) => ExprKind::Ident(n),
            Tok::Constructor => ExprKind::Ident(b"constructor".to_vec()),
            Tok::This => ExprKind::This,
            Tok::Parent => ExprKind::Parent,
            Tok::Vargc => ExprKind::Vargc,
            Tok::Vargv => {
                self.expect_op("[")?;
                let idx = self.comma_expr()?;
                self.expect_op("]")?;
                ExprKind::Vargv(Box::new(idx))
            }
            Tok::Op("::") => ExprKind::Root(self.ident()?),
            Tok::Op("(") => {
                let e = self.comma_expr()?;
                self.expect_op(")")?;
                return Ok(e);
            }
            Tok::Op("[") => {
                let mut items = Vec::new();
                while !self.is_op("]") {
                    items.push(self.expression()?);
                    let _ = self.eat_op(",");
                }
                self.advance();
                ExprKind::Array(items)
            }
            Tok::Op("{") => {
                let mut slots = Vec::new();
                while !self.is_op("}") {
                    if self.at_eof() {
                        return Err(self.err("expected '}'"));
                    }
                    if *self.tok() == Tok::Function {
                        let fline = self.line();
                        self.advance();
                        let name = self.ident()?;
                        let def = self.function_body(Some(name.clone()), fline)?;
                        slots.push((Expr::new(ExprKind::Str(name), fline), Expr::new(ExprKind::Function(Rc::new(def)), fline)));
                    } else {
                        let key = self.table_key()?;
                        self.expect_op("=")?;
                        let value = self.expression()?;
                        slots.push((key, value));
                    }
                    let _ = self.eat_op(",");
                }
                self.advance();
                ExprKind::Table(slots)
            }
            Tok::Function => {
                let def = self.function_body(None, line)?;
                ExprKind::Function(Rc::new(def))
            }
            Tok::Class => {
                let def = self.class_body()?;
                ExprKind::Class(Rc::new(def))
            }
            other => {
                self.pos -= 1;
                return Err(self.err(format!("expression expected, found {}", describe(&other))));
            }
        };
        Ok(Expr::new(kind, line))
    }
}

fn is_lvalue(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::Ident(_) | ExprKind::Field(..) | ExprKind::Index(..) | ExprKind::Root(_) | ExprKind::Vargv(_))
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Ident(n) => format!("'{}'", String::from_utf8_lossy(n)),
        Tok::Op(o) => format!("'{o}'"),
        Tok::Eof => "end of script".into(),
        other => format!("{other:?}").to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_forms() {
        let src = br#"
            local a = 1, b
            function Foo::Bar(x, y = 2, ...) { return x + y * vargc }
            class Thing extends Base </ doc = "x" /> { constructor(v) { value = v } value = null static count = 0 }
            if (a == 1 && b in t) print("x"); else { a <- 3 }
            foreach (k, v in t) { switch (v) { case 1: break; default: a++ } }
            try { throw "oops" } catch (e) { ::print(e) }
            t.x[3](1, 2)
            local f = function(a) { yield a }
            enum E { A, B = 5 }
            const C = -1
        "#;
        let stmts = parse(src, "t").unwrap();
        assert_eq!(stmts.len(), 10);
    }

    #[test]
    fn statements_need_separators() {
        assert!(parse(b"a = 1 b = 2", "t").is_err());
        assert!(parse(b"a = 1\nb = 2", "t").is_ok());
    }
}
