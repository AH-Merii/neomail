//! An attachment's real type, from its first bytes rather than from what the sender declared,
//! and whether it is an executable or a script.

/// The type of content neomail cannot tell.
pub const UNKNOWN: &str = "application/octet-stream";
/// A nested message's type.
pub const MESSAGE: &str = "message/rfc822";
const ZIP: &str = "application/zip";
const OLE: &str = "application/x-ole-storage";
const PE: &str = "application/vnd.microsoft.portable-executable";
const ELF: &str = "application/x-executable";
const MACH_O: &str = "application/x-mach-binary";
const SHORTCUT: &str = "application/x-ms-shortcut";
/// Any file starting with `#!`: a script for whatever interpreter that line names.
const SCRIPT: &str = "text/x-script";
const JAR: &str = "application/java-archive";
const APK: &str = "application/vnd.android.package-archive";

/// Signatures at the start of the content, longest first where one is a prefix of another.
const MAGIC: &[(&[u8], &str)] = &[
    (b"%PDF-", "application/pdf"),
    (b"\x89PNG\r\n\x1a\n", "image/png"),
    (b"\xff\xd8\xff", "image/jpeg"),
    (b"GIF87a", "image/gif"),
    (b"GIF89a", "image/gif"),
    (b"II*\0", "image/tiff"),
    (b"MM\0*", "image/tiff"),
    (b"\0\0\x01\0", "image/vnd.microsoft.icon"),
    (b"\x1f\x8b", "application/gzip"),
    (b"7z\xbc\xaf\x27\x1c", "application/x-7z-compressed"),
    (b"Rar!\x1a\x07", "application/vnd.rar"),
    (b"\xfd7zXZ\0", "application/x-xz"),
    (b"BZh", "application/x-bzip2"),
    (b"\x7fELF", ELF),
    (b"\xfe\xed\xfa\xce", MACH_O),
    (b"\xfe\xed\xfa\xcf", MACH_O),
    (b"\xce\xfa\xed\xfe", MACH_O),
    (b"\xcf\xfa\xed\xfe", MACH_O),
    (b"\xca\xfe\xba\xbe", MACH_O),
    (b"\0asm", "application/wasm"),
    (b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1", OLE),
    (b"L\0\0\0\x01\x14\x02\0", SHORTCUT),
    (b"{\\rtf", "application/rtf"),
    (b"%!PS", "application/postscript"),
    (b"OggS", "audio/ogg"),
    (b"fLaC", "audio/flac"),
    (b"ID3", "audio/mpeg"),
    (b"#!", SCRIPT),
    (b"BEGIN:VCALENDAR", "text/calendar"),
    (b"BEGIN:VCARD", "text/vcard"),
];

/// Types whose content is text, which `real_type` cannot tell apart from one another reliably.
const TEXTUAL: &[&str] = &[
    "application/json",
    "application/xml",
    "application/javascript",
    "application/x-sh",
    "application/x-shellscript",
    "application/yaml",
    "application/x-yaml",
    "application/toml",
    "application/sql",
    "message/rfc822",
];

/// The type `data` is, from its content, or [`UNKNOWN`].
pub fn real_type(data: &[u8]) -> &'static str {
    if let Some((_, t)) = MAGIC.iter().find(|(m, _)| data.starts_with(m)) {
        return t;
    }
    if data.starts_with(b"PK\x03\x04") {
        return zip_kind(data);
    }
    if is_pe(data) {
        return PE;
    }
    if data.len() >= 12 && data.starts_with(b"RIFF") {
        match &data[8..12] {
            b"WEBP" => return "image/webp",
            b"WAVE" => return "audio/wav",
            b"AVI " => return "video/x-msvideo",
            _ => {}
        }
    }
    if data.len() >= 12 && &data[4..8] == b"ftyp" {
        return match &data[8..12] {
            b"heic" | b"heix" | b"mif1" => "image/heic",
            b"avif" => "image/avif",
            b"qt  " => "video/quicktime",
            b"M4A " => "audio/mp4",
            _ => "video/mp4",
        };
    }
    text_kind(data).unwrap_or(UNKNOWN)
}

/// `MZ` with the offset at 0x3C pointing at `PE\0\0`: a Windows program or library. `MZ` alone is
/// too short a signature, as plenty of text starts with it.
fn is_pe(data: &[u8]) -> bool {
    if !data.starts_with(b"MZ") || data.len() < 0x40 {
        return false;
    }
    let offset = u32::from_le_bytes([data[0x3c], data[0x3d], data[0x3e], data[0x3f]]) as usize;
    data.get(offset..offset.saturating_add(4)) == Some(b"PE\0\0")
}

/// Which kind of ZIP: an OpenDocument or EPUB names its type in a first entry `mimetype`; Office,
/// Java and Android archives are told by the names in the archive.
fn zip_kind(data: &[u8]) -> &'static str {
    let name_len = data
        .get(26..28)
        .map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let extra_len = data
        .get(28..30)
        .map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as usize);
    if data.get(30..30 + name_len) == Some(b"mimetype") {
        let start = 30 + name_len + extra_len;
        let declared = data.get(start..data.len().min(start + 80)).unwrap_or(&[]);
        for t in [
            "application/vnd.oasis.opendocument.text",
            "application/vnd.oasis.opendocument.spreadsheet",
            "application/vnd.oasis.opendocument.presentation",
            "application/epub+zip",
        ] {
            if declared.starts_with(t.as_bytes()) {
                return t;
            }
        }
    }
    // The names sit in the local headers and in the central directory at the end.
    let head = &data[..data.len().min(64 * 1024)];
    let tail = &data[data.len().saturating_sub(256 * 1024)..];
    let has = |name: &[u8]| contains(head, name) || contains(tail, name);
    if has(b"AndroidManifest.xml") {
        APK
    } else if has(b"META-INF/MANIFEST.MF") {
        JAR
    } else if has(b"word/document.xml") {
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    } else if has(b"xl/workbook.xml") {
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
    } else if has(b"ppt/presentation.xml") {
        "application/vnd.openxmlformats-officedocument.presentationml.presentation"
    } else {
        ZIP
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// HTML, SVG, XML or plain text: UTF-8 (a character cut at the end of the sample is fine) without
/// NUL bytes.
fn text_kind(data: &[u8]) -> Option<&'static str> {
    let sample = &data[..data.len().min(8192)];
    if sample.is_empty() || sample.contains(&0) {
        return None;
    }
    match std::str::from_utf8(sample) {
        Ok(_) => {}
        Err(e) if e.error_len().is_none() && data.len() > sample.len() => {}
        Err(_) => return None,
    }
    let lower: Vec<u8> = sample
        .iter()
        .take(1024)
        .map(u8::to_ascii_lowercase)
        .collect();
    let start = lower.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&lower);
    let start = &start[start.iter().take_while(|b| b.is_ascii_whitespace()).count()..];
    Some(
        if start.starts_with(b"<!doctype html") || start.starts_with(b"<html") {
            "text/html"
        } else if start.starts_with(b"<svg")
            || (start.starts_with(b"<?xml") && contains(start, b"<svg"))
        {
            "image/svg+xml"
        } else if start.starts_with(b"<?xml") {
            "application/xml"
        } else {
            "text/plain"
        },
    )
}

fn is_textual(t: &str) -> bool {
    t.starts_with("text/") || t.ends_with("+xml") || t.ends_with("+json") || TEXTUAL.contains(&t)
}

/// One spelling per type, for the aliases mail programs send.
fn canonical(t: &str) -> &str {
    match t {
        "image/jpg" | "image/pjpeg" => "image/jpeg",
        "image/x-png" => "image/png",
        "image/x-icon" => "image/vnd.microsoft.icon",
        "application/x-pdf" => "application/pdf",
        "application/x-zip-compressed" | "application/x-zip" => ZIP,
        "application/x-gzip" => "application/gzip",
        "application/x-rar-compressed" | "application/x-rar" => "application/vnd.rar",
        "audio/mp3" | "audio/x-mpeg" => "audio/mpeg",
        "audio/x-wav" | "audio/wave" => "audio/wav",
        "application/x-msdownload" | "application/x-dosexec" | "application/x-msdos-program" => PE,
        "application/x-elf" | "application/x-sharedlib" => ELF,
        t => t,
    }
}

/// Containers of one format that other types are made of: a `.docx` is a ZIP, a `.doc` an OLE
/// file, a `.m4a` an MP4. A sender naming the container, or another member, is not lying.
const FAMILIES: &[&[&str]] = &[
    &[
        ZIP,
        JAR,
        APK,
        "application/epub+zip",
        "application/vnd.oasis.opendocument.text",
        "application/vnd.oasis.opendocument.spreadsheet",
        "application/vnd.oasis.opendocument.presentation",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ],
    &[
        OLE,
        "application/msword",
        "application/vnd.ms-excel",
        "application/vnd.ms-powerpoint",
        "application/vnd.ms-outlook",
        "application/x-msi",
    ],
    &[
        "video/mp4",
        "audio/mp4",
        "audio/x-m4a",
        "video/quicktime",
        "video/3gpp",
        "image/heic",
        "image/heif",
        "image/avif",
    ],
];

/// Whether content of type `real` fits a claim (a declared type, or one an extension implies).
/// Unknown content fits everything; text fits any claim but a binary format neomail recognises.
pub fn fits(claimed: &str, real: &str) -> bool {
    let (claimed, real) = (canonical(claimed), canonical(real));
    if real == UNKNOWN || claimed == UNKNOWN || claimed == real {
        return true;
    }
    if is_textual(real) {
        return is_textual(claimed) || !recognised_binary(claimed);
    }
    FAMILIES
        .iter()
        .any(|f| f.contains(&claimed) && f.contains(&real))
}

/// A binary type [`real_type`] can return or that an extension maps to.
fn recognised_binary(t: &str) -> bool {
    let known = MAGIC
        .iter()
        .map(|(_, t)| *t)
        .chain(EXTENSIONS.iter().map(|(_, t)| *t));
    !is_textual(t) && (known.into_iter().any(|k| k == t) || FAMILIES.iter().any(|f| f.contains(&t)))
}

/// The type a file extension (lower case, without the dot) stands for.
const EXTENSIONS: &[(&str, &str)] = &[
    ("pdf", "application/pdf"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
    ("ico", "image/vnd.microsoft.icon"),
    ("heic", "image/heic"),
    ("avif", "image/avif"),
    ("svg", "image/svg+xml"),
    ("zip", ZIP),
    ("gz", "application/gzip"),
    ("7z", "application/x-7z-compressed"),
    ("rar", "application/vnd.rar"),
    ("xz", "application/x-xz"),
    ("bz2", "application/x-bzip2"),
    (
        "docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
    (
        "xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
    (
        "pptx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ),
    ("odt", "application/vnd.oasis.opendocument.text"),
    ("ods", "application/vnd.oasis.opendocument.spreadsheet"),
    ("odp", "application/vnd.oasis.opendocument.presentation"),
    ("epub", "application/epub+zip"),
    ("jar", JAR),
    ("apk", APK),
    ("doc", "application/msword"),
    ("xls", "application/vnd.ms-excel"),
    ("ppt", "application/vnd.ms-powerpoint"),
    ("msi", "application/x-msi"),
    ("rtf", "application/rtf"),
    ("mp3", "audio/mpeg"),
    ("ogg", "audio/ogg"),
    ("flac", "audio/flac"),
    ("wav", "audio/wav"),
    ("mp4", "video/mp4"),
    ("m4a", "audio/mp4"),
    ("mov", "video/quicktime"),
    ("exe", PE),
    ("dll", PE),
    ("lnk", SHORTCUT),
    ("wasm", "application/wasm"),
    ("html", "text/html"),
    ("htm", "text/html"),
    ("txt", "text/plain"),
    ("csv", "text/csv"),
    ("ics", "text/calendar"),
    ("vcf", "text/vcard"),
    ("eml", "message/rfc822"),
];

/// The type a name's extension stands for, if neomail knows it.
pub fn extension_type(name: &str) -> Option<&'static str> {
    let ext = extension(name)?;
    EXTENSIONS.iter().find(|(e, _)| *e == ext).map(|(_, t)| *t)
}

/// An extension for a file of type `t` that has no name, with its dot, or `""`.
pub fn extension_for(t: &str) -> String {
    EXTENSIONS
        .iter()
        .find(|(_, k)| *k == t)
        .map_or_else(String::new, |(e, _)| format!(".{e}"))
}

/// The lower-case text after the name's last dot, when there is one after the first character.
fn extension(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then(|| ext.to_ascii_lowercase())
}

/// Extensions a desktop runs, installs or interprets rather than shows: programs, installers,
/// shortcuts and scripts for Windows, macOS and Linux.
const RISKY_EXTENSIONS: &[&str] = &[
    "exe",
    "com",
    "scr",
    "pif",
    "cpl",
    "dll",
    "sys",
    "msi",
    "msp",
    "msix",
    "appx",
    "bat",
    "cmd",
    "ps1",
    "psm1",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "hta",
    "lnk",
    "reg",
    "scf",
    "url",
    "inf",
    "jar",
    "apk",
    "app",
    "command",
    "pkg",
    "dmg",
    "sh",
    "bash",
    "zsh",
    "csh",
    "ksh",
    "fish",
    "py",
    "pyw",
    "pl",
    "rb",
    "php",
    "lua",
    "run",
    "bin",
    "appimage",
    "desktop",
    "deb",
    "rpm",
    "elf",
    "out",
    "docm",
    "dotm",
    "xlsm",
    "xltm",
    "xlam",
    "xll",
    "pptm",
    "potm",
    "ppsm",
    "ppam",
    "msc",
    "vb",
    "chm",
    "jnlp",
    "sct",
    "flatpakref",
    "flatpak",
    "snap",
];

/// Executables, installers, shortcuts and scripts, by content.
const RISKY_TYPES: &[&str] = &[PE, ELF, MACH_O, SHORTCUT, SCRIPT, JAR, APK];

/// Whether opening the file could run code: an executable or script by its real type, or by the
/// extension of the name it would be saved under.
pub fn is_risky(real: &str, name: &str) -> bool {
    RISKY_TYPES.contains(&real)
        || extension(name).is_some_and(|e| RISKY_EXTENSIONS.contains(&e.as_str()))
}
