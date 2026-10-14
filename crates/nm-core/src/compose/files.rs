//! Files attached to an outgoing message (`--attach`): their names, their types and the check
//! for a forgotten attachment. Building them into the message is [`super::Draft::build`]'s job.
//! This is the compose side only; reading the attachments of cached mail lives elsewhere.
//!
//! - A file's name is its last path part, without control or bidirectional formatting characters,
//!   so no folder of the sender's and nothing that reorders text reaches the recipient. A name
//!   that is not ASCII is sent as RFC 2231 says (<https://www.rfc-editor.org/rfc/rfc2231>), which
//!   mail-builder writes for any parameter value that needs it.
//! - A file's type comes from its first bytes (its magic number); the extension decides only
//!   when the bytes name no type, or name a container that several types share (ZIP for Office
//!   and OpenDocument files, OLE for old Office files). Text that names no type is
//!   `text/plain` when it is UTF-8, else `application/octet-stream`.

use std::fmt;

use crate::attachment::sniff::{UNKNOWN, is_textual, real_type};

/// The name a file goes by when it has none left after cleaning.
pub const FALLBACK_NAME: &str = "attachment";

/// A file to send: its cleaned name, its type and its bytes. `Debug` leaves the bytes out.
#[derive(Clone, PartialEq, Eq)]
pub struct AttachedFile {
    name: String,
    media_type: &'static str,
    bytes: Vec<u8>,
}

impl AttachedFile {
    /// A file named `name` (a path or a bare name; only its last part is kept) holding `bytes`.
    pub fn new(name: &str, bytes: Vec<u8>) -> Self {
        let name = safe_name(name);
        let media_type = media_type(&bytes, &name);
        Self {
            name,
            media_type,
            bytes,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Its type, such as `application/pdf`.
    pub fn media_type(&self) -> &'static str {
        self.media_type
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Its size in bytes, before any encoding.
    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }
}

impl fmt::Debug for AttachedFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "AttachedFile({:?}, {}, {} bytes)",
            self.name,
            self.media_type,
            self.bytes.len()
        )
    }
}

/// Characters that change the order or direction text is shown in: the bidirectional marks,
/// embeddings, overrides and isolates (Unicode UAX #9), which can make `exe.pdf` look like
/// `fdp.exe`.
fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// `name`'s last part after `/` or `\`, without control or bidirectional formatting characters and
/// trimmed; [`FALLBACK_NAME`] when nothing usable is left.
pub fn safe_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = last
        .chars()
        .filter(|c| !c.is_control() && !is_bidi_control(*c))
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        FALLBACK_NAME.to_owned()
    } else {
        cleaned.to_owned()
    }
}

/// The type an extension names, lower-cased, if neomail knows it.
fn by_extension(name: &str) -> Option<&'static str> {
    let (_, extension) = name.rsplit_once('.')?;
    Some(match extension.to_ascii_lowercase().as_str() {
        "txt" | "text" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "ics" => "text/calendar",
        "vcf" => "text/vcard",
        "json" => "application/json",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "tif" | "tiff" => "image/tiff",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "tar" => "application/x-tar",
        "7z" => "application/x-7z-compressed",
        "doc" => "application/msword",
        "xls" => "application/vnd.ms-excel",
        "ppt" => "application/vnd.ms-powerpoint",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "epub" => "application/epub+zip",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        _ => return None,
    })
}

/// Formats stored inside a ZIP container, which the bytes alone cannot tell apart.
const IN_ZIP: [&str; 7] = [
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "application/vnd.oasis.opendocument.text",
    "application/vnd.oasis.opendocument.spreadsheet",
    "application/vnd.oasis.opendocument.presentation",
    "application/epub+zip",
];

/// Formats stored inside an OLE compound file: the Office formats before 2007.
const IN_OLE: [&str; 3] = [
    "application/msword",
    "application/vnd.ms-excel",
    "application/vnd.ms-powerpoint",
];

/// A file's type, from its content as [`real_type`] tells the attachments neomail reads; its
/// extension decides only where the content cannot: unknown bytes, a container several formats
/// share (a ZIP, an OLE file), or plain text whose extension names a kind of text.
pub fn media_type(bytes: &[u8], name: &str) -> &'static str {
    let named = by_extension(name);
    match (real_type(bytes), named) {
        ("application/zip", Some(n)) if IN_ZIP.contains(&n) => n,
        ("application/x-ole-storage", Some(n)) if IN_OLE.contains(&n) => n,
        ("application/x-ole-storage", _) => UNKNOWN,
        ("text/plain", Some(n)) if is_textual(n) => n,
        (UNKNOWN, Some(n)) => n,
        (found, _) => found,
    }
}

/// The words that say a file is meant to come with the message.
const ATTACH_WORDS: [&str; 6] = [
    "attach",
    "attached",
    "attaches",
    "attaching",
    "attachment",
    "attachments",
];

/// Whether `text` says something is attached: one of [`ATTACH_WORDS`] as a whole word, in any
/// case, outside a quoted line (one starting with `>`, which is someone else's text).
pub fn mentions_attachment(text: &str) -> bool {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('>'))
        .flat_map(|line| line.split(|c: char| !c.is_alphanumeric()))
        .any(|word| ATTACH_WORDS.iter().any(|w| word.eq_ignore_ascii_case(w)))
}
