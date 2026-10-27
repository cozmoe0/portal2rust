//! Console text handling: splitting text into commands and commands into
//! arguments the way the original command buffer does.

/// Characters that always form a token of their own outside quotes.
const BREAK_CHARS: &[char] = &['{', '}', '(', ')', '\'', ':'];

/// Splits console text into commands at `;` and line breaks. Quotes protect
/// `;` but never span lines; `//` starts a comment that runs to the end of
/// the line.
pub fn split_commands(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    let mut flush = |cur: &mut String| {
        let t = cur.trim();
        if !t.is_empty() {
            out.push(t.to_owned());
        }
        cur.clear();
    };
    while let Some(c) = chars.next() {
        match c {
            '\n' | '\r' => {
                quoted = false;
                flush(&mut cur);
            }
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ';' if !quoted => flush(&mut cur),
            '/' if !quoted && chars.peek() == Some(&'/') => {
                while chars.peek().is_some_and(|&n| n != '\n') {
                    chars.next();
                }
            }
            _ => cur.push(c),
        }
    }
    flush(&mut cur);
    out
}

/// One command: its arguments, and the raw text after the command name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandArgs {
    pub argv: Vec<String>,
    pub args_raw: String,
}

impl CommandArgs {
    pub fn parse(text: &str) -> Self {
        let text = text.trim();
        let argv = tokenize(text);
        // The raw argument string starts after the first token.
        let args_raw = match text.strip_prefix('"') {
            Some(rest) => rest.split_once('"').map(|(_, r)| r).unwrap_or(""),
            None => text.split_once(char::is_whitespace).map(|(_, r)| r).unwrap_or(""),
        }
        .trim()
        .to_owned();
        Self { argv, args_raw }
    }

    pub fn name(&self) -> &str {
        self.argv.first().map(String::as_str).unwrap_or("")
    }

    /// Argument `i` (0 is the command name), or "" when absent.
    pub fn arg(&self, i: usize) -> &str {
        self.argv.get(i).map(String::as_str).unwrap_or("")
    }

    /// Number of tokens including the command name.
    pub fn len(&self) -> usize {
        self.argv.len()
    }

    pub fn is_empty(&self) -> bool {
        self.argv.is_empty()
    }

    pub fn arg_f32(&self, i: usize) -> Option<f32> {
        self.argv.get(i).and_then(|s| s.trim().parse().ok())
    }

    /// The raw argument text with one pair of surrounding quotes removed.
    pub fn rest(&self) -> &str {
        let r = self.args_raw.as_str();
        r.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(r)
    }
}

fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let Some(&c) = chars.peek() else { break };
        if c == '"' {
            chars.next();
            let mut tok = String::new();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                tok.push(c);
            }
            out.push(tok);
        } else if BREAK_CHARS.contains(&c) {
            chars.next();
            out.push(c.to_string());
        } else {
            let mut tok = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || c == '"' || BREAK_CHARS.contains(&c) {
                    break;
                }
                tok.push(c);
                chars.next();
            }
            out.push(tok);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_semicolons_and_lines_outside_quotes() {
        let cmds = split_commands("bind \"x\" \"echo a; echo b\"; echo c\n// comment\necho d // trailing");
        assert_eq!(cmds, vec!["bind \"x\" \"echo a; echo b\"", "echo c", "echo d"]);
    }

    #[test]
    fn quotes_do_not_span_lines() {
        assert_eq!(split_commands("echo \"a;\nb;c"), vec!["echo \"a;", "b", "c"]);
    }

    #[test]
    fn tokenizes_quotes_and_break_characters() {
        let a = CommandArgs::parse("setpos 1 -2.5 3");
        assert_eq!(a.argv, vec!["setpos", "1", "-2.5", "3"]);
        assert_eq!(a.args_raw, "1 -2.5 3");
        let b = CommandArgs::parse("bind \"MOUSE1\" \"+attack\"");
        assert_eq!(b.argv, vec!["bind", "MOUSE1", "+attack"]);
        let c = CommandArgs::parse("script printl(\"hi\")");
        assert_eq!(c.argv, vec!["script", "printl", "(", "hi", ")"]);
        assert_eq!(c.args_raw, "printl(\"hi\")");
        let d = CommandArgs::parse("cc_lang \"\"");
        assert_eq!(d.argv, vec!["cc_lang", ""]);
    }

    #[test]
    fn rest_strips_one_pair_of_quotes() {
        assert_eq!(CommandArgs::parse("echo \"hello world\"").rest(), "hello world");
        assert_eq!(CommandArgs::parse("echo hello world").rest(), "hello world");
    }
}
