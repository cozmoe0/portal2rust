//! An interpreter for Squirrel 2.2, the scripting language of the original
//! game's map scripts, written from the language reference.
//!
//! The dialect is 2.2's: free variables are captured by value when a closure
//! is created, `parent`, `delegate`, `vargc` and `vargv` exist, plain calls
//! pass the caller's `this`, and unknown names are looked up in `this` and
//! then the root table. Tables keep the original's slot layout so iterating
//! them yields keys in the same order.
//!
//! With the `serde` feature, [`snapshot`] saves a whole VM and restores it
//! into a freshly set-up one (saved games).

pub mod ast;
pub mod compiler;
pub mod error;
pub mod format;
pub mod lexer;
pub mod parser;
#[cfg(feature = "serde")]
pub mod snapshot;
pub mod stdlib;
pub mod table;
pub mod value;
pub mod vm;

pub use error::{Error, Result};
pub use value::{NativeFn, ObjRef, SqStr, Value};
pub use vm::{Ctx, Vm};
