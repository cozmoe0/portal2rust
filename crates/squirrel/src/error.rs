//! Compile errors and runtime exceptions.

use std::fmt;

use crate::value::Value;

#[derive(Clone, Debug)]
pub enum Error {
    Compile { msg: String, source: String, line: u32 },
    /// A thrown value (an error message string for runtime errors) with the
    /// call stack at the point it was raised.
    Runtime { value: Value, message: String, stack: Vec<String> },
}

impl Error {
    pub fn compile(msg: impl Into<String>, source: &str, line: u32) -> Self {
        Error::Compile { msg: msg.into(), source: source.to_owned(), line }
    }

    /// Runtime error with a message; the VM fills in the stack.
    pub fn runtime(msg: impl Into<String>) -> Self {
        let message = msg.into();
        Error::Runtime { value: Value::str(&message), message, stack: Vec::new() }
    }

    pub fn thrown(value: Value, message: String) -> Self {
        Error::Runtime { value, message, stack: Vec::new() }
    }

    pub fn message(&self) -> &str {
        match self {
            Error::Compile { msg, .. } => msg,
            Error::Runtime { message, .. } => message,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Compile { msg, source, line } => write!(f, "{source} line = ({line}) : {msg}"),
            Error::Runtime { message, stack, .. } => {
                write!(f, "AN ERROR HAS OCCURED [{message}]")?;
                for s in stack {
                    write!(f, "\n  {s}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
