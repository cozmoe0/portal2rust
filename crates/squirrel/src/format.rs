//! Number formatting compatible with the C library the original was built
//! with: `%g` for float to string conversion (three-digit exponents) and the
//! printf subset used by `format()`.

/// C `%.{precision}g`.
pub fn format_g(x: f64, precision: usize) -> String {
    let spec = Spec { precision: Some(precision), conv: 'g', ..Spec::default() };
    format_float(x, &spec)
}

#[derive(Clone, Debug, Default)]
pub struct Spec {
    pub left: bool,
    pub plus: bool,
    pub space: bool,
    pub alt: bool,
    pub zero: bool,
    pub width: usize,
    pub precision: Option<usize>,
    pub conv: char,
}

impl Spec {
    /// Parses the part after `%` up to and including the conversion
    /// character; returns the spec and the number of bytes consumed.
    pub fn parse(s: &[u8]) -> Option<(Spec, usize)> {
        let mut spec = Spec::default();
        let mut i = 0;
        while i < s.len() {
            match s[i] {
                b'-' => spec.left = true,
                b'+' => spec.plus = true,
                b' ' => spec.space = true,
                b'#' => spec.alt = true,
                b'0' => spec.zero = true,
                _ => break,
            }
            i += 1;
        }
        let mut w = 0;
        while i < s.len() && s[i].is_ascii_digit() {
            w = w * 10 + (s[i] - b'0') as usize;
            i += 1;
        }
        spec.width = w;
        if i < s.len() && s[i] == b'.' {
            i += 1;
            let mut p = 0;
            while i < s.len() && s[i].is_ascii_digit() {
                p = p * 10 + (s[i] - b'0') as usize;
                i += 1;
            }
            spec.precision = Some(p);
        }
        // Length modifiers are accepted and ignored.
        while i < s.len() && matches!(s[i], b'l' | b'h' | b'L' | b'I') {
            i += 1;
        }
        if i >= s.len() {
            return None;
        }
        spec.conv = s[i] as char;
        Some((spec, i + 1))
    }

    fn pad(&self, sign: &str, body: String, numeric: bool) -> String {
        let len = sign.len() + body.len();
        if len >= self.width {
            return format!("{sign}{body}");
        }
        let fill = self.width - len;
        if self.left {
            format!("{sign}{body}{}", " ".repeat(fill))
        } else if self.zero && numeric {
            format!("{sign}{}{body}", "0".repeat(fill))
        } else {
            format!("{}{sign}{body}", " ".repeat(fill))
        }
    }
}

fn sign_of(neg: bool, spec: &Spec) -> &'static str {
    if neg {
        "-"
    } else if spec.plus {
        "+"
    } else if spec.space {
        " "
    } else {
        ""
    }
}

pub fn format_int(v: i64, spec: &Spec) -> String {
    let (neg, mag) = (v < 0, v.unsigned_abs());
    let mut body = match spec.conv {
        'x' => format!("{:x}", v as u32),
        'X' => format!("{:X}", v as u32),
        'o' => format!("{:o}", v as u32),
        'u' => format!("{}", v as u32),
        'c' => return spec.pad("", ((v as u8) as char).to_string(), false),
        _ => mag.to_string(),
    };
    let signed = matches!(spec.conv, 'd' | 'i');
    if let Some(p) = spec.precision {
        while body.len() < p {
            body.insert(0, '0');
        }
    }
    if spec.alt && matches!(spec.conv, 'x' | 'X') && v != 0 {
        body = format!("{}{body}", if spec.conv == 'x' { "0x" } else { "0X" });
    }
    let zero_ok = spec.precision.is_none();
    let sign = if signed { sign_of(neg, spec) } else { "" };
    let mut s = spec.clone();
    s.zero = spec.zero && zero_ok;
    s.pad(sign, body, true)
}

/// `%e` mantissa and exponent parts with `p` decimals, C style.
fn exp_parts(x: f64, p: usize) -> (String, i32) {
    let s = format!("{:.*e}", p, x);
    let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
    (m.to_owned(), e.parse().unwrap_or(0))
}

fn exp_suffix(e: i32, upper: bool) -> String {
    // The original's C runtime always prints at least three exponent digits.
    format!("{}{}{:03}", if upper { 'E' } else { 'e' }, if e < 0 { '-' } else { '+' }, e.unsigned_abs())
}

fn strip_zeros(s: &mut String) {
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
}

pub fn format_float(x: f64, spec: &Spec) -> String {
    let neg = x.is_sign_negative() && !(x == 0.0 && !x.is_sign_negative());
    let sign = sign_of(neg, spec);
    let a = x.abs();
    let upper = spec.conv.is_ascii_uppercase();
    if a.is_nan() {
        return spec.pad(sign, if neg { "1.#IND".into() } else { "1.#QNAN".into() }, false);
    }
    if a.is_infinite() {
        return spec.pad(sign, "1.#INF".into(), false);
    }
    let p = spec.precision.unwrap_or(6);
    let body = match spec.conv.to_ascii_lowercase() {
        'f' => {
            let mut s = format!("{:.*}", p, a);
            if spec.alt && p == 0 {
                s.push('.');
            }
            s
        }
        'e' => {
            let (m, e) = exp_parts(a, p);
            format!("{m}{}", exp_suffix(e, upper))
        }
        _ => {
            let p = if p == 0 { 1 } else { p };
            if a == 0.0 {
                let mut s = "0".to_owned();
                if spec.alt {
                    s = format!("{:.*}", p - 1, 0.0);
                }
                s
            } else {
                let (_, e) = exp_parts(a, p - 1);
                if e < -4 || e >= p as i32 {
                    let (mut m, e) = exp_parts(a, p - 1);
                    if !spec.alt {
                        strip_zeros(&mut m);
                    }
                    format!("{m}{}", exp_suffix(e, upper))
                } else {
                    let decimals = (p as i32 - 1 - e).max(0) as usize;
                    let mut s = format!("{:.*}", decimals, a);
                    if !spec.alt {
                        strip_zeros(&mut s);
                    }
                    s
                }
            }
        }
    };
    spec.pad(sign, body, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g_matches_c_runtime() {
        assert_eq!(format_g(1.0, 6), "1");
        assert_eq!(format_g(0.5, 6), "0.5");
        assert_eq!(format_g(100000.0, 6), "100000");
        assert_eq!(format_g(1000000.0, 6), "1e+006");
        assert_eq!(format_g(0.0001, 6), "0.0001");
        assert_eq!(format_g(0.00001, 6), "1e-005");
        assert_eq!(format_g(-2.25, 6), "-2.25");
        assert_eq!(format_g(1.0 / 3.0, 6), "0.333333");
        assert_eq!(format_g(123456789.0, 6), "1.23457e+008");
    }

    #[test]
    #[allow(clippy::approx_constant, reason = "3.14159 is just a number to format here")]
    fn printf_subset() {
        let spec = |s: &str| Spec::parse(s.as_bytes()).unwrap().0;
        assert_eq!(format_int(42, &spec("5d")), "   42");
        assert_eq!(format_int(-42, &spec("05d")), "-0042");
        assert_eq!(format_int(255, &spec("x")), "ff");
        assert_eq!(format_float(3.14159, &spec(".2f")), "3.14");
        assert_eq!(format_float(2.5, &spec("8.3f")), "   2.500");
        assert_eq!(format_float(1234.5, &spec("e")), "1.234500e+003");
    }
}
