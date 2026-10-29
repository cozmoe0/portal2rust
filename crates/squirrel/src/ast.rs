//! Syntax tree produced by the parser and consumed by the compiler.

use std::rc::Rc;

pub type Name = Vec<u8>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Shl,
    Shr,
    UShr,
    BitAnd,
    BitOr,
    BitXor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    InstanceOf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
    TypeOf,
    Clone,
    Resume,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssignOp {
    Set,
    NewSlot,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Null,
    Bool(bool),
    Int(i32),
    Float(f32),
    Str(Name),
    /// A bare name: local, outer, constant, or a slot of `this` / the root.
    Ident(Name),
    This,
    /// The delegate of `this`.
    Parent,
    /// `expr.parent`: the delegate of a table or the base of a class.
    ParentOf(Box<Expr>),
    /// `::name`
    Root(Name),
    Base,
    Vargc,
    Vargv(Box<Expr>),
    Field(Box<Expr>, Name),
    Index(Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    Assign(AssignOp, Box<Expr>, Box<Expr>),
    IncDec {
        target: Box<Expr>,
        delta: i32,
        prefix: bool,
    },
    Delete(Box<Expr>),
    Table(Vec<(Expr, Expr)>),
    Array(Vec<Expr>),
    Function(Rc<FuncDef>),
    Class(Rc<ClassDef>),
    /// `delegate parent : table`
    Delegate(Box<Expr>, Box<Expr>),
    Comma(Vec<Expr>),
}

impl Expr {
    pub fn new(kind: ExprKind, line: u32) -> Self {
        Self { kind, line }
    }
}

#[derive(Clone, Debug)]
pub struct FuncDef {
    pub name: Option<Name>,
    /// Parameter names with optional default values.
    pub params: Vec<(Name, Option<Expr>)>,
    pub varargs: bool,
    pub body: Vec<Stmt>,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct ClassMember {
    pub key: Expr,
    pub value: Expr,
    pub is_static: bool,
    pub attrs: Option<Expr>,
}

#[derive(Clone, Debug)]
pub struct ClassDef {
    pub base: Option<Expr>,
    pub attrs: Option<Expr>,
    pub members: Vec<ClassMember>,
}

#[derive(Clone, Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub enum StmtKind {
    Expr(Expr),
    Local(Vec<(Name, Option<Expr>)>),
    LocalFunction(Name, Rc<FuncDef>),
    /// `function a::b::c() {}`: assigns into a slot path.
    Function(Vec<Name>, Rc<FuncDef>),
    /// `class a.b extends c {}`: the target expression and the class.
    Class(Expr, Rc<ClassDef>),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    While(Expr, Box<Stmt>),
    DoWhile(Box<Stmt>, Expr),
    For(Option<Box<Stmt>>, Option<Expr>, Option<Expr>, Box<Stmt>),
    Foreach {
        key: Option<Name>,
        value: Name,
        iter: Expr,
        body: Box<Stmt>,
    },
    Switch(Expr, Vec<(Expr, Vec<Stmt>)>, Option<Vec<Stmt>>),
    Break,
    Continue,
    Return(Option<Expr>),
    Yield(Option<Expr>),
    Block(Vec<Stmt>),
    Try(Box<Stmt>, Name, Box<Stmt>),
    Throw(Expr),
    Const(Name, Expr),
    Enum(Name, Vec<(Name, Option<Expr>)>),
    Empty,
}
