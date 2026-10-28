//! Valve `.vfont` files: a TrueType font with its bytes scrambled.
//!
//! The file ends with `VFONT1`, preceded by the salt: some bytes and then
//! their count, the count byte included. Every font byte before the salt is
//! XORed with a running key;
//! after each byte the key becomes the stored byte plus 167 (mod 256). The
//! key for the first byte is derived from the salt; since every installed
//! font is TrueType (`00 01 00 00` sfnt version), the first plain byte is 0
//! and the starting key is simply the first stored byte.

const MAGIC: &[u8] = b"VFONT1";

/// The plain font bytes, or `None` when `data` is not a `.vfont`.
pub fn decode(data: &[u8]) -> Option<Vec<u8>> {
    let end = data.len().checked_sub(MAGIC.len())?;
    if &data[end..] != MAGIC || end == 0 {
        return None;
    }
    let salt_len = data[end - 1] as usize;
    let body = end.checked_sub(salt_len.max(1))?;
    let mut out = Vec::with_capacity(body);
    let mut key = *data.first()?;
    for &b in &data[..body] {
        out.push(b ^ key);
        key = b.wrapping_add(167);
    }
    (out.starts_with(&[0, 1, 0, 0]) || out.starts_with(b"OTTO")).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_other_files() {
        assert!(decode(b"not a font").is_none());
        assert!(decode(b"VFONT1").is_none());
    }

    #[test]
    fn decodes_installed_fonts() {
        let Some(root) = crate::fs::GameFileSystem::find_install() else { return };
        let Ok(fs) = crate::fs::GameFileSystem::portal2(root) else { return };
        for name in ["universc", "univercb", "univercl", "consola"] {
            let Some(data) = fs.read(&format!("vgui/fonts/{name}.vfont")) else { continue };
            let font = decode(&data).unwrap_or_else(|| panic!("{name} did not decode"));
            // numTables is non-zero and the first table record has a tag.
            let tables = u16::from_be_bytes([font[4], font[5]]) as usize;
            assert!(tables > 0 && font[12..16].iter().all(|c| c.is_ascii_graphic()), "{name}");
            // Every table lies inside the decoded bytes.
            for i in 0..tables {
                let r = &font[12 + i * 16..28 + i * 16];
                let end = u32::from_be_bytes([r[8], r[9], r[10], r[11]]) + u32::from_be_bytes([r[12], r[13], r[14], r[15]]);
                assert!(end as usize <= font.len(), "{name}: table {i} past the end");
            }
        }
    }
}
