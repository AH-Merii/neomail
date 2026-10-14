//! The file name an attachment is saved under: the sender's name made safe for a file system and
//! a terminal, or `attachment-INDEX` when nothing of it is left.

use super::sniff;

/// The longest safe name in bytes: below the 255 bytes most file systems allow, leaving room for
/// a `-INDEX` that keeps names unique and for the temporary name an overwrite writes first.
pub const MAX_NAME_BYTES: usize = 200;

/// The longest extension kept when a long name is cut.
const MAX_EXTENSION_BYTES: usize = 16;

/// Characters dropped from a name: control characters (C0, DEL and C1), the line and paragraph
/// separators, every bidi control (which can make `exe.pdf` read as `fdp.exe`), and invisible
/// characters such as zero-width spaces and the byte order mark.
fn dropped(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}'
                | '\u{200B}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{2069}'
                | '\u{FEFF}'
        )
}

/// `name` made safe to save: only its last path component (after `/` or `\`), without dropped
/// characters, with characters other systems forbid (`:*?"<>|`) as `_`, without leading dots,
/// dashes and spaces or trailing dots and spaces, and cut to [`MAX_NAME_BYTES`] keeping a short
/// extension. With nothing left, `attachment-INDEX` and the extension of the real type `real`.
/// A nested message (`message/rfc822`) always ends in `.eml`, so a mail app opens it.
pub fn safe_name(name: Option<&str>, index: usize, real: &str) -> String {
    let base = name.unwrap_or("").rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .filter(|c| !dropped(*c))
        .map(|c| if ":*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    let trimmed = cleaned
        .trim_start_matches(|c: char| c == '.' || c == '-' || c.is_whitespace())
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    if trimmed.is_empty() {
        return format!("attachment-{index}{}", sniff::extension_for(real));
    }
    let ext = sniff::extension_for(real);
    if real == sniff::MESSAGE && sniff::extension_type(trimmed) != Some(sniff::MESSAGE) {
        return cap(&format!("{trimmed}{ext}"), MAX_NAME_BYTES);
    }
    cap(trimmed, MAX_NAME_BYTES)
}

/// `name` cut to at most `max` bytes on a character boundary, keeping its extension when that is
/// short.
fn cap(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_owned();
    }
    let ext = match name.rfind('.') {
        Some(dot) if dot > 0 && name.len() - dot <= MAX_EXTENSION_BYTES => &name[dot..],
        _ => "",
    };
    let mut end = max - ext.len();
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    let stem = name[..end].trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    format!("{stem}{ext}")
}

/// `name` with `-INDEX` before its extension, for a second attachment with the same safe name.
pub fn numbered(name: &str, index: usize) -> String {
    let suffix = format!("-{index}");
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 && name.len() - dot <= MAX_EXTENSION_BYTES => name.split_at(dot),
        _ => (name, ""),
    };
    let mut end = stem
        .len()
        .min(MAX_NAME_BYTES.saturating_sub(suffix.len() + ext.len()));
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}{ext}", &stem[..end])
}
