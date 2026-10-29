//! Small backtracking regular expression engine for the `regexp` class:
//! literals, `.`, `^`, `$`, classes, groups (`(...)`, `(?:...)`),
//! alternation, greedy `* + ? {n,m}` and the class escapes `\w \s \d \x \c
//! \p \l \u \a \b` (upper case negates).

use std::cell::RefCell;

use crate::error::{Error, Result};

#[derive(Clone, Debug)]
enum ClassItem {
    Byte(u8),
    Range(u8, u8),
    Escape(u8),
}

#[derive(Clone, Debug)]
enum Node {
    Byte(u8),
    Any,
    Start,
    End,
    Escape(u8),
    WordBoundary(bool),
    Class(Vec<ClassItem>, bool),
    Group(Box<Node>, Option<usize>),
    Alt(Vec<Node>),
    Seq(Vec<Node>),
    Repeat(Box<Node>, usize, Option<usize>),
}

pub struct Regex {
    root: Node,
    groups: usize,
}

type Caps = RefCell<Vec<Option<(usize, usize)>>>;

fn escape_matches(e: u8, c: u8) -> bool {
    let m = match e.to_ascii_lowercase() {
        b'w' => c.is_ascii_alphanumeric() || c == b'_',
        b's' => c.is_ascii_whitespace(),
        b'd' => c.is_ascii_digit(),
        b'x' => c.is_ascii_hexdigit(),
        b'c' => c.is_ascii_control(),
        b'p' => c.is_ascii_punctuation(),
        b'l' => c.is_ascii_lowercase(),
        b'u' => c.is_ascii_uppercase(),
        b'a' => c.is_ascii_alphabetic(),
        _ => return c == e,
    };
    if e.is_ascii_uppercase() { !m } else { m }
}

fn literal_escape(c: u8) -> Option<u8> {
    Some(match c {
        b't' => b'\t',
        b'n' => b'\n',
        b'r' => b'\r',
        b'f' => 0x0c,
        b'v' => 0x0b,
        _ => return None,
    })
}

struct Parser<'a> {
    s: &'a [u8],
    pos: usize,
    groups: usize,
}

impl Parser<'_> {
    fn err(&self) -> Error {
        Error::runtime("invalid regular expression")
    }

    fn alt(&mut self) -> Result<Node> {
        let mut branches = vec![self.seq()?];
        while self.s.get(self.pos) == Some(&b'|') {
            self.pos += 1;
            branches.push(self.seq()?);
        }
        Ok(if branches.len() == 1 { branches.pop().expect("one") } else { Node::Alt(branches) })
    }

    fn seq(&mut self) -> Result<Node> {
        let mut items = Vec::new();
        while let Some(&c) = self.s.get(self.pos) {
            if c == b'|' || c == b')' {
                break;
            }
            let atom = self.atom()?;
            items.push(self.quantified(atom)?);
        }
        Ok(Node::Seq(items))
    }

    fn number(&mut self) -> Option<usize> {
        let start = self.pos;
        while self.s.get(self.pos).is_some_and(u8::is_ascii_digit) {
            self.pos += 1;
        }
        std::str::from_utf8(&self.s[start..self.pos]).ok()?.parse().ok()
    }

    fn quantified(&mut self, atom: Node) -> Result<Node> {
        let (min, max) = match self.s.get(self.pos) {
            Some(b'*') => (0, None),
            Some(b'+') => (1, None),
            Some(b'?') => (0, Some(1)),
            Some(b'{') => {
                self.pos += 1;
                let min = self.number().ok_or_else(|| self.err())?;
                let max = if self.s.get(self.pos) == Some(&b',') {
                    self.pos += 1;
                    self.number()
                } else {
                    Some(min)
                };
                if self.s.get(self.pos) != Some(&b'}') {
                    return Err(self.err());
                }
                (min, max)
            }
            _ => return Ok(atom),
        };
        self.pos += 1;
        Ok(Node::Repeat(Box::new(atom), min, max))
    }

    fn atom(&mut self) -> Result<Node> {
        let c = self.s[self.pos];
        self.pos += 1;
        Ok(match c {
            b'.' => Node::Any,
            b'^' => Node::Start,
            b'$' => Node::End,
            b'(' => {
                let capture = if self.s[self.pos..].starts_with(b"?:") {
                    self.pos += 2;
                    None
                } else {
                    self.groups += 1;
                    Some(self.groups)
                };
                let inner = self.alt()?;
                if self.s.get(self.pos) != Some(&b')') {
                    return Err(self.err());
                }
                self.pos += 1;
                Node::Group(Box::new(inner), capture)
            }
            b'[' => {
                let neg = self.s.get(self.pos) == Some(&b'^');
                if neg {
                    self.pos += 1;
                }
                let mut items = Vec::new();
                loop {
                    let Some(&c) = self.s.get(self.pos) else { return Err(self.err()) };
                    self.pos += 1;
                    if c == b']' && !items.is_empty() {
                        break;
                    }
                    let item = if c == b'\\' {
                        let e = *self.s.get(self.pos).ok_or_else(|| self.err())?;
                        self.pos += 1;
                        match literal_escape(e) {
                            Some(b) => ClassItem::Byte(b),
                            None if e.is_ascii_alphabetic() => ClassItem::Escape(e),
                            None => ClassItem::Byte(e),
                        }
                    } else if self.s.get(self.pos) == Some(&b'-') && self.s.get(self.pos + 1).is_some_and(|&n| n != b']') {
                        let hi = self.s[self.pos + 1];
                        self.pos += 2;
                        ClassItem::Range(c, hi)
                    } else {
                        ClassItem::Byte(c)
                    };
                    items.push(item);
                }
                Node::Class(items, neg)
            }
            b'\\' => {
                let e = *self.s.get(self.pos).ok_or_else(|| self.err())?;
                self.pos += 1;
                match e {
                    b'b' => Node::WordBoundary(true),
                    b'B' => Node::WordBoundary(false),
                    _ => match literal_escape(e) {
                        Some(b) => Node::Byte(b),
                        None if e.is_ascii_alphabetic() => Node::Escape(e),
                        None => Node::Byte(e),
                    },
                }
            }
            other => Node::Byte(other),
        })
    }
}

impl Regex {
    pub fn new(pattern: &[u8]) -> Result<Self> {
        let mut p = Parser { s: pattern, pos: 0, groups: 0 };
        let root = p.alt()?;
        if p.pos != pattern.len() {
            return Err(p.err());
        }
        Ok(Self { root, groups: p.groups })
    }

    pub fn groups(&self) -> usize {
        self.groups
    }

    pub fn full_match(&self, s: &[u8]) -> bool {
        let caps = RefCell::new(vec![None; self.groups + 1]);
        m(&self.root, s, 0, &caps, &|p| p == s.len())
    }

    /// First match at or after `start`: whole match then each group.
    pub fn search(&self, s: &[u8], start: usize) -> Option<Vec<Option<(usize, usize)>>> {
        for begin in start..=s.len() {
            let caps = RefCell::new(vec![None; self.groups + 1]);
            let end = std::cell::Cell::new(0);
            if m(&self.root, s, begin, &caps, &|p| {
                end.set(p);
                true
            }) {
                let mut c = caps.into_inner();
                c[0] = Some((begin, end.get()));
                return Some(c);
            }
        }
        None
    }
}

fn is_word(c: Option<&u8>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

fn m(node: &Node, s: &[u8], pos: usize, caps: &Caps, k: &dyn Fn(usize) -> bool) -> bool {
    match node {
        Node::Byte(b) => s.get(pos) == Some(b) && k(pos + 1),
        Node::Any => pos < s.len() && k(pos + 1),
        Node::Start => pos == 0 && k(pos),
        Node::End => pos == s.len() && k(pos),
        Node::Escape(e) => s.get(pos).is_some_and(|c| escape_matches(*e, *c)) && k(pos + 1),
        Node::WordBoundary(want) => {
            let at = is_word(pos.checked_sub(1).and_then(|p| s.get(p))) != is_word(s.get(pos));
            at == *want && k(pos)
        }
        Node::Class(items, neg) => {
            let Some(&c) = s.get(pos) else { return false };
            let hit = items.iter().any(|i| match i {
                ClassItem::Byte(b) => *b == c,
                ClassItem::Range(lo, hi) => (*lo..=*hi).contains(&c),
                ClassItem::Escape(e) => escape_matches(*e, c),
            });
            hit != *neg && k(pos + 1)
        }
        Node::Group(inner, capture) => m(inner, s, pos, caps, &|p| match capture {
            Some(i) => {
                let old = caps.borrow()[*i];
                caps.borrow_mut()[*i] = Some((pos, p));
                if k(p) {
                    true
                } else {
                    caps.borrow_mut()[*i] = old;
                    false
                }
            }
            None => k(p),
        }),
        Node::Alt(branches) => branches.iter().any(|b| m(b, s, pos, caps, k)),
        Node::Seq(items) => seq(items, s, pos, caps, k),
        Node::Repeat(inner, min, max) => repeat(inner, *min, *max, 0, s, pos, caps, k),
    }
}

fn seq(items: &[Node], s: &[u8], pos: usize, caps: &Caps, k: &dyn Fn(usize) -> bool) -> bool {
    match items.split_first() {
        None => k(pos),
        Some((first, rest)) => m(first, s, pos, caps, &|p| seq(rest, s, p, caps, k)),
    }
}

#[allow(clippy::too_many_arguments)]
fn repeat(
    inner: &Node,
    min: usize,
    max: Option<usize>,
    count: usize,
    s: &[u8],
    pos: usize,
    caps: &Caps,
    k: &dyn Fn(usize) -> bool,
) -> bool {
    if max.is_none_or(|mx| count < mx) && m(inner, s, pos, caps, &|p| p != pos && repeat(inner, min, max, count + 1, s, p, caps, k)) {
        return true;
    }
    count >= min && k(pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_common_patterns() {
        let re = Regex::new(b"^On.*Output$").unwrap();
        assert!(re.full_match(b"OnTurretOutput"));
        assert!(!re.full_match(b"OnTurret"));
        let re = Regex::new(b"(\\d+)-([a-z]+)").unwrap();
        let caps = re.search(b"xx 12-ab yy", 0).unwrap();
        assert_eq!(caps[0], Some((3, 8)));
        assert_eq!(caps[1], Some((3, 5)));
        assert_eq!(caps[2], Some((6, 8)));
        assert!(Regex::new(b"a{2,3}").unwrap().full_match(b"aaa"));
        assert!(!Regex::new(b"a{2,3}").unwrap().full_match(b"aaaa"));
        assert!(Regex::new(b"cat|dog").unwrap().full_match(b"dog"));
    }
}
