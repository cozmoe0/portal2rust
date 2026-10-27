//! Text decoding helpers. Source data files are a mix of UTF-8, Latin-1 and
//! UTF-16LE (localisation and caption sources), sometimes with a BOM.

pub fn decode_text(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        return decode_utf16le(&bytes[2..]);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        // Older files are Windows-1252; Latin-1 is close enough for keys and paths.
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

pub fn decode_utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

/// Normalises a game path: lowercase, forward slashes, no leading `./` or `/`.
pub fn normalize_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut last_slash = false;
    for ch in path.chars() {
        let c = if ch == '\\' { '/' } else { ch.to_ascii_lowercase() };
        if c == '/' {
            if last_slash || out.is_empty() {
                last_slash = true;
                continue;
            }
            last_slash = true;
        } else {
            last_slash = false;
        }
        out.push(c);
    }
    while let Some(rest) = out.strip_prefix("./") {
        out = rest.to_owned();
    }
    if out.ends_with('/') {
        out.pop();
    }
    out
}

/// Strips a trailing extension if present.
pub fn strip_extension(path: &str) -> &str {
    match path.rfind('.') {
        Some(dot) if !path[dot..].contains('/') => &path[..dot],
        _ => path,
    }
}

/// Returns the lowercase extension without the dot.
pub fn extension(path: &str) -> &str {
    match path.rfind('.') {
        Some(dot) if !path[dot..].contains('/') => &path[dot + 1..],
        _ => "",
    }
}
