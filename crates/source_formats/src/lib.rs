//! Readers for the Source engine file formats used by Portal 2.
//!
//! Everything in this crate is engine-agnostic: it turns bytes from the game's
//! own data files into plain Rust structures. Nothing here contains game data;
//! all content is read at runtime from a local installation.

pub mod bsp;
pub mod fgd;
pub mod fs;
pub mod kv;
pub mod mdl;
pub mod phy;
pub mod reader;
pub mod studio;
pub mod text;
pub mod vmt;
pub mod vpk;
pub mod vtf;
pub mod vhv;
pub mod vtx;
pub mod vvd;
pub mod zip;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unexpected end of data")]
    Eof,
    #[error("bad magic: expected {expected}, found {found:?}")]
    Magic { expected: &'static str, found: Vec<u8> },
    #[error("unsupported version {0}")]
    Version(i64),
    #[error("{0}")]
    Invalid(String),
    #[error("file not found: {0}")]
    NotFound(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Invalid(msg.into()))
}
