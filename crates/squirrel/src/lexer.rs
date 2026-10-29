//! Tokenizer for Squirrel 2.2 source text.
//!
//! Source files are byte strings (the game's scripts are not UTF-8), so the
//! lexer works on bytes and string literals stay byte strings.

use crate::error::{Error, Result};

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(Vec<u8>),
    Str(Vec<u8>),
    Int(i32),
    Float(f32),
    // Keywords.
    While,
    Do,
    If,
    Else,
    Break,
    Continue,
    Return,
    Null,
    Function,
    Local,
    For,
    Foreach,
    In,
    Typeof,
    Delegate,
    Delete,
    Try,
    Catch,
    Throw,
    Clone,
    Yield,
    Resume,
    Switch,
    Case,
    Default,
    This,
    Parent,
    Class,
    Extends,
    Constructor,
    Instanceof,
    Vargc,
    Vargv,
    True,
    False,
    Static,
    Enum,
    Const,
    Base,
    // Operators and punctuation.
    Op(&'static str),
    Eof,
}

/// A token with the line it starts on and whether a line break preceded it
/// (statements may end at a line break).
#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
    pub newline_before: bool,
}

fn keyword(word: &[u8]) -> Option<Tok> {
    Some(match word {
        b"while" => Tok::While,
        b"do" => Tok::Do,
        b"if" => Tok::If,
        b"else" => Tok::Else,
        b"break" => Tok::Break,
        b"continue" => Tok::Continue,
        b"return" => Tok::Return,
        b"null" => Tok::Null,
        b"function" => Tok::Function,
        b"local" => Tok::Local,
        b"for" => Tok::For,
        b"foreach" => Tok::Foreach,
        b"in" => Tok::In,
        b"typeof" => Tok::Typeof,
        b"delegate" => Tok::Delegate,
        b"delete" => Tok::Delete,
        b"try" => Tok::Try,
        b"catch" => Tok::Catch,
        b"throw" => Tok::Throw,
        b"clone" => Tok::Clone,
        b"yield" => Tok::Yield,
        b"resume" => Tok::Resume,
        b"switch" => Tok::Switch,
        b"case" => Tok::Case,
        b"default" => Tok::Default,
        b"this" => Tok::This,
        b"parent" => Tok::Parent,
        b"class" => Tok::Class,
        b"extends" => Tok::Extends,
        b"constructor" => Tok::Constructor,
        b"instanceof" => Tok::Instanceof,
        b"vargc" => Tok::Vargc,
        b"vargv" => Tok::Vargv,
        b"true" => Tok::True,
        b"false" => Tok::False,
        b"static" => Tok::Static,
        b"enum" => Tok::Enum,
        b"const" => Tok::Const,
        _ => return None,
    })
}

/// Operators, longest first so greedy matching works.
const OPS: &[&str] = &[
    ">>>", "</", "/>", "<-", "::", "==", "!=", "<=", ">=", "&&", "||", "++", "--", "+=", "-=", "*=", "/=", "%=", "<<", ">>", "...", "+",
    "-", "*", "/", "%", "=", "<", ">", "!", "~", "&", "|", "^", "(", ")", "[", "]", "{", "}", ",", ";", ":", "?", ".", "@",
];

pub fn tokenize(src: &[u8], source_name: &str) -> Result<Vec<Token>> {
    let mut lx = Lexer { src, pos: 0, line: 1, name: source_name };
    let mut out = Vec::new();
    loop {
        let newline_before = lx.skip_space()?;
        let line = lx.line;
        let tok = lx.next()?;
        let eof = tok == Tok::Eof;
        out.push(Token { tok, line, newline_before });
        if eof {
            break;
        }
    }
    Ok(out)
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: u32,
    name: &'a str,
}

impl Lexer<'_> {
    fn peek(&self, off: usize) -> u8 {
        self.src.get(self.pos + off).copied().unwrap_or(0)
    }

    fn err(&self, msg: impl Into<String>) -> Error {
        Error::compile(msg, self.name, self.line)
    }

    /// Skips whitespace and comments; returns whether a line break was seen.
    fn skip_space(&mut self) -> Result<bool> {
        let mut newline = false;
        loop {
            match self.peek(0) {
                b'\n' => {
                    newline = true;
                    self.line += 1;
                    self.pos += 1;
                }
                b' ' | b'\t' | b'\r' | 0x0b | 0x0c => self.pos += 1,
                b'#' => self.skip_line(),
                b'/' if self.peek(1) == b'/' => self.skip_line(),
                b'/' if self.peek(1) == b'*' => {
                    self.pos += 2;
                    loop {
                        if self.pos >= self.src.len() {
                            return Err(self.err("missing \"*/\" in comment"));
                        }
                        if self.peek(0) == b'*' && self.peek(1) == b'/' {
                            self.pos += 2;
                            break;
                        }
                        if self.peek(0) == b'\n' {
                            newline = true;
                            self.line += 1;
                        }
                        self.pos += 1;
                    }
                }
                _ => return Ok(newline),
            }
        }
    }

    fn skip_line(&mut self) {
        while self.pos < self.src.len() && self.peek(0) != b'\n' {
            self.pos += 1;
        }
    }

    fn next(&mut self) -> Result<Tok> {
        let c = self.peek(0);
        if self.pos >= self.src.len() {
            return Ok(Tok::Eof);
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = self.pos;
            while self.peek(0).is_ascii_alphanumeric() || self.peek(0) == b'_' {
                self.pos += 1;
            }
            let word = &self.src[start..self.pos];
            return Ok(keyword(word).unwrap_or_else(|| Tok::Ident(word.to_vec())));
        }
        if c.is_ascii_digit() || (c == b'.' && self.peek(1).is_ascii_digit()) {
            return self.number();
        }
        if c == b'"' {
            self.pos += 1;
            return self.string(b'"', false);
        }
        if c == b'@' && self.peek(1) == b'"' {
            self.pos += 2;
            return self.string(b'"', true);
        }
        if c == b'\'' {
            self.pos += 1;
            let v = match self.peek(0) {
                b'\\' => {
                    self.pos += 1;
                    self.escape()?
                }
                0 => return Err(self.err("unfinished character constant")),
                ch => {
                    self.pos += 1;
                    ch
                }
            };
            if self.peek(0) != b'\'' {
                return Err(self.err("invalid character constant"));
            }
            self.pos += 1;
            return Ok(Tok::Int(v as i8 as i32));
        }
        for op in OPS {
            if self.src[self.pos..].starts_with(op.as_bytes()) {
                self.pos += op.len();
                return Ok(Tok::Op(op));
            }
        }
        Err(self.err(format!("unexpected character '{}'", c as char)))
    }

    fn number(&mut self) -> Result<Tok> {
        let start = self.pos;
        if self.peek(0) == b'0' && matches!(self.peek(1), b'x' | b'X') {
            self.pos += 2;
            let s = self.pos;
            while self.peek(0).is_ascii_hexdigit() {
                self.pos += 1;
            }
            let digits = std::str::from_utf8(&self.src[s..self.pos]).unwrap_or("0");
            let v = u64::from_str_radix(digits, 16).map_err(|_| self.err("invalid hex number"))?;
            return Ok(Tok::Int(v as u32 as i32));
        }
        // Octal literals start with 0 followed by digits.
        if self.peek(0) == b'0' && self.peek(1).is_ascii_digit() {
            let s = self.pos;
            while self.peek(0).is_ascii_digit() {
                self.pos += 1;
            }
            if self.peek(0) != b'.' && !matches!(self.peek(0), b'e' | b'E') {
                let digits = std::str::from_utf8(&self.src[s..self.pos]).unwrap_or("0");
                let v = u64::from_str_radix(digits, 8).map_err(|_| self.err("invalid octal number"))?;
                return Ok(Tok::Int(v as u32 as i32));
            }
            self.pos = s;
        }
        let mut is_float = false;
        while self.peek(0).is_ascii_digit() {
            self.pos += 1;
        }
        if self.peek(0) == b'.' && self.peek(1) != b'.' {
            is_float = true;
            self.pos += 1;
            while self.peek(0).is_ascii_digit() {
                self.pos += 1;
            }
        }
        if matches!(self.peek(0), b'e' | b'E')
            && (self.peek(1).is_ascii_digit() || (matches!(self.peek(1), b'+' | b'-') && self.peek(2).is_ascii_digit()))
        {
            // Exponents are only accepted on numbers that already have a '.'.
            if !is_float {
                return Err(self.err("invalid numeric format"));
            }
            self.pos += 2;
            while self.peek(0).is_ascii_digit() {
                self.pos += 1;
            }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).unwrap_or("0");
        if is_float {
            Ok(Tok::Float(text.parse::<f64>().map_err(|_| self.err("invalid float"))? as f32))
        } else {
            // Decimal literals wrap to 32 bits like the original.
            let v: u64 = text.parse().unwrap_or(u64::MAX);
            Ok(Tok::Int(v as u32 as i32))
        }
    }

    fn escape(&mut self) -> Result<u8> {
        let c = self.peek(0);
        self.pos += 1;
        Ok(match c {
            b't' => b'\t',
            b'a' => 0x07,
            b'b' => 0x08,
            b'n' => b'\n',
            b'r' => b'\r',
            b'v' => 0x0b,
            b'f' => 0x0c,
            b'0' => 0,
            b'\\' => b'\\',
            b'"' => b'"',
            b'\'' => b'\'',
            b'x' => {
                let s = self.pos;
                while self.pos - s < 2 && self.peek(0).is_ascii_hexdigit() {
                    self.pos += 1;
                }
                let digits = std::str::from_utf8(&self.src[s..self.pos]).unwrap_or("");
                u8::from_str_radix(digits, 16).map_err(|_| self.err("hexadecimal number expected"))?
            }
            _ => return Err(self.err("unrecognised escaper char")),
        })
    }

    fn string(&mut self, delim: u8, verbatim: bool) -> Result<Tok> {
        let mut out = Vec::new();
        loop {
            let c = self.peek(0);
            if self.pos >= self.src.len() {
                return Err(self.err("unfinished string"));
            }
            if c == delim {
                // `""` inside a verbatim string is a literal quote.
                if verbatim && self.peek(1) == delim {
                    out.push(delim);
                    self.pos += 2;
                    continue;
                }
                self.pos += 1;
                break;
            }
            if c == b'\n' {
                if !verbatim {
                    return Err(self.err("newline in a constant"));
                }
                self.line += 1;
            }
            if c == b'\\' && !verbatim {
                self.pos += 1;
                out.push(self.escape()?);
                continue;
            }
            out.push(c);
            self.pos += 1;
        }
        Ok(Tok::Str(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        tokenize(s.as_bytes(), "test").unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn lexes_literals_and_operators() {
        assert_eq!(
            toks("local x <- 0x10 + 1.5e2 // c\n'a' @\"q\"\"q\" \"t\\n\""),
            vec![
                Tok::Local,
                Tok::Ident(b"x".to_vec()),
                Tok::Op("<-"),
                Tok::Int(16),
                Tok::Op("+"),
                Tok::Float(150.0),
                Tok::Int(97),
                Tok::Str(b"q\"q".to_vec()),
                Tok::Str(b"t\n".to_vec()),
                Tok::Eof
            ]
        );
        assert_eq!(toks("a>>>=b"), vec![Tok::Ident(b"a".to_vec()), Tok::Op(">>>"), Tok::Op("="), Tok::Ident(b"b".to_vec()), Tok::Eof]);
    }

    #[test]
    fn tracks_line_breaks() {
        let t = tokenize(b"a\n/* x\n y */ b c", "t").unwrap();
        assert!(!t[0].newline_before && t[1].newline_before && !t[2].newline_before);
        assert_eq!(t[1].line, 3);
    }
}
