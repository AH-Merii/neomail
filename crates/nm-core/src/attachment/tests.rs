#![allow(clippy::unwrap_used)]

use super::name::{MAX_NAME_BYTES, safe_name};
use super::sniff::real_type;
use super::*;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
const PDF: &[u8] = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n";
const ELF_BYTES: &[u8] = b"\x7fELF\x02\x01\x01\0\0\0\0\0";

/// A multipart/mixed message with a text body and one part per `(headers, body)`; bodies go out
/// in base64.
fn message(parts: &[(&str, &[u8])]) -> Vec<u8> {
    use base64::Engine;
    let mut m = b"From: a@example.com\r\nSubject: s\r\nMIME-Version: 1.0\r\n\
        Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\n\
        Content-Type: text/plain\r\n\r\nbody\r\n"
        .to_vec();
    for (headers, body) in parts {
        let encoded = base64::engine::general_purpose::STANDARD.encode(body);
        m.extend_from_slice(
            format!("--b\r\n{headers}\r\nContent-Transfer-Encoding: base64\r\n\r\n{encoded}\r\n")
                .as_bytes(),
        );
    }
    m.extend_from_slice(b"--b--\r\n");
    m
}

fn file(ct: &str, name: &str) -> String {
    format!("Content-Type: {ct}\r\nContent-Disposition: attachment; filename=\"{name}\"")
}

fn infos(raw: &[u8]) -> Vec<Info> {
    extract(raw).unwrap().into_iter().map(|e| e.info).collect()
}

#[test]
fn lists_each_attachment_by_index_with_both_types_and_the_decoded_content() {
    let raw = message(&[
        (&file("application/pdf", "report.pdf"), PDF),
        (&file("image/png", "chart.png"), PNG),
    ]);
    let got = extract(&raw).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].info.index, 1);
    assert_eq!(got[0].info.real_type, "application/pdf");
    assert_eq!(got[0].data, PDF);
    assert_eq!(got[0].info.size, PDF.len());
    assert_eq!(got[1].info.index, 2);
    assert_eq!(got[1].info.declared_type, "image/png");
    assert_eq!(got[1].info.real_type, "image/png");
    assert!(!got[1].info.type_mismatch && !got[1].info.executable);
}

#[test]
fn a_real_type_that_contradicts_the_declared_type_or_the_extension_is_a_mismatch() {
    let cases: &[(&str, &str, &[u8], bool)] = &[
        // Declared type, name, content, mismatch.
        ("application/pdf", "report.pdf", PNG, true),
        ("application/octet-stream", "photo.pdf", PNG, true),
        (
            "image/png",
            "notes.png",
            b"plain text, not an image\n",
            true,
        ),
        (
            "image/jpg",
            "photo.jpeg",
            b"\xff\xd8\xff\xe0\0\x10JFIF",
            false,
        ),
        ("application/octet-stream", "report.pdf", PDF, false),
        (
            "application/x-zip-compressed",
            "a.zip",
            b"PK\x03\x04\x14\0\0\0",
            false,
        ),
        ("text/csv", "data.csv", b"a,b\n1,2\n", false),
        (
            "application/pdf",
            "report.pdf",
            b"\x00\x01\x02 unknown",
            false,
        ),
    ];
    for (ct, name, body, mismatch) in cases {
        let got = infos(&message(&[(&file(ct, name), body)]));
        assert_eq!(got[0].type_mismatch, *mismatch, "{ct} {name}");
    }
}

#[test]
fn office_files_are_told_from_plain_zips_and_fit_a_declared_zip() {
    let mut docx = b"PK\x03\x04\x14\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x13\0\0\0".to_vec();
    docx.extend_from_slice(b"[Content_Types].xml...PK\x01\x02word/document.xml");
    assert_eq!(
        real_type(&docx),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    );
    let got = infos(&message(&[(
        &file("application/zip", "letter.docx"),
        &docx,
    )]));
    assert!(!got[0].type_mismatch);
}

#[test]
fn text_beginning_with_mz_is_not_a_windows_program() {
    assert_eq!(
        real_type(b"MZ is how this note starts, and it is only text.\n"),
        "text/plain"
    );
    let mut pe = vec![0u8; 0x80];
    pe[..2].copy_from_slice(b"MZ");
    pe[0x3c] = 0x40;
    pe[0x40..0x44].copy_from_slice(b"PE\0\0");
    assert_eq!(
        real_type(&pe),
        "application/vnd.microsoft.portable-executable"
    );
}

#[test]
fn executables_and_scripts_are_flagged_by_content_or_by_name() {
    let cases: &[(&str, &[u8], bool)] = &[
        ("photo.jpg", ELF_BYTES, true),
        ("invoice.pdf.exe", PDF, true),
        ("run-me", b"#!/bin/sh\necho hi\n", true),
        ("macro.XLSM", b"PK\x03\x04\x14\0\0\0", true),
        ("form.xlam", b"PK\x03\x04\x14\0\0\0", true),
        ("app.flatpakref", b"[Flatpak Ref]\nName=x\n", true),
        ("setup.desktop", b"[Desktop Entry]\nExec=sh\n", true),
        ("report.pdf", PDF, false),
        ("notes.txt", b"#not a shebang\n", false),
    ];
    for (name, body, executable) in cases {
        let got = infos(&message(&[(&file("application/octet-stream", name), body)]));
        assert_eq!(got[0].executable, *executable, "{name}");
    }
}

#[test]
fn hostile_names_become_safe_file_names() {
    let cases: &[(Option<&str>, &str)] = &[
        (Some("../../.bashrc"), "bashrc"),
        (Some("..\\..\\Windows\\evil.dll"), "evil.dll"),
        (Some("/etc/passwd"), "passwd"),
        (Some(".."), "attachment-3.pdf"),
        (Some("-rf"), "rf"),
        (Some("--help.txt"), "help.txt"),
        (Some(" . hidden"), "hidden"),
        (Some("invoice\u{202E}fdp.exe"), "invoicefdp.exe"),
        (Some("bell\x07\x1b[31mred\r\n.txt"), "bell[31mred.txt"),
        (Some("a\u{200B}b\u{FEFF}.txt"), "ab.txt"),
        (Some("what?<is>:this|*.txt"), "what__is__this__.txt"),
        (Some("trailing.pdf. . "), "trailing.pdf"),
        (Some(""), "attachment-3.pdf"),
        (None, "attachment-3.pdf"),
    ];
    for (name, want) in cases {
        assert_eq!(safe_name(*name, 3, "application/pdf"), *want, "{name:?}");
    }
    assert_eq!(
        safe_name(None, 2, "application/octet-stream"),
        "attachment-2"
    );
}

#[test]
fn a_long_name_is_cut_on_a_character_boundary_keeping_its_extension() {
    let long = format!("{}.pdf", "é".repeat(300));
    let got = safe_name(Some(&long), 1, "application/pdf");
    assert!(got.len() <= MAX_NAME_BYTES, "{}", got.len());
    assert!(got.ends_with(".pdf") && got.starts_with('é'));
}

#[test]
fn attachments_with_the_same_safe_name_get_their_index_added() {
    let raw = message(&[
        (&file("application/pdf", "a/report.pdf"), PDF),
        (&file("application/pdf", "b/REPORT.pdf"), PDF),
    ]);
    let got = infos(&raw);
    assert_eq!(got[0].safe_name, "report.pdf");
    assert_eq!(got[1].safe_name, "REPORT-2.pdf");

    // The numbered name can itself be taken; it is then numbered again.
    let raw = message(&[
        (&file("text/plain", "a.txt"), b"one"),
        (&file("text/plain", "a-3.txt"), b"two"),
        (&file("text/plain", "a.txt"), b"three"),
    ]);
    let names: Vec<String> = infos(&raw).into_iter().map(|i| i.safe_name).collect();
    assert_eq!(names, ["a.txt", "a-3.txt", "a-3-2.txt"]);
}

#[test]
fn a_part_mail_parser_reads_as_text_is_saved_with_the_bytes_that_were_sent() {
    let latin1: &[u8] = b"caf\xe9 \x93quoted\x94\r\n";
    let raw = message(&[
        // Declared text, but a PNG.
        (&file("text/plain", "logo.png"), PNG),
        // No Content-Type at all.
        (
            "Content-Disposition: attachment; filename=\"chart.png\"",
            PNG,
        ),
        // A charset other than UTF-8 is not converted.
        (
            &file("text/plain; charset=windows-1252", "menu.txt"),
            latin1,
        ),
    ]);
    let got = extract(&raw).unwrap();
    assert_eq!(got[0].data, PNG);
    assert_eq!(got[0].info.real_type, "image/png");
    assert_eq!(got[0].info.size, PNG.len());
    assert_eq!(got[1].data, PNG);
    assert_eq!(got[1].info.declared_type, "application/octet-stream");
    assert_eq!(got[2].data, latin1);

    // Quoted-printable and unencoded text parts too.
    let raw = b"From: a@example.com\r\nMIME-Version: 1.0\r\n\
        Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\n\
        Content-Type: text/plain\r\n\r\nbody\r\n--b\r\n\
        Content-Type: text/plain; charset=iso-8859-1\r\n\
        Content-Disposition: attachment; filename=\"qp.txt\"\r\n\
        Content-Transfer-Encoding: quoted-printable\r\n\r\ncaf=E9 =\r\nau lait\r\n--b\r\n\
        Content-Type: text/plain; charset=iso-8859-1\r\n\
        Content-Disposition: attachment; filename=\"raw.txt\"\r\n\
        Content-Transfer-Encoding: 8bit\r\n\r\ncaf\xe9\r\nline two\r\n--b--\r\n";
    let got = extract(raw).unwrap();
    assert_eq!(got[0].data, b"caf\xe9 au lait");
    assert_eq!(got[1].data, b"caf\xe9\r\nline two");
}

#[test]
fn a_part_an_html_body_points_at_by_cid_is_inline() {
    let raw = b"From: a@example.com\r\nMIME-Version: 1.0\r\n\
        Content-Type: multipart/related; boundary=\"r\"\r\n\r\n--r\r\n\
        Content-Type: text/html\r\n\r\n<p>Logo: <img src=\"CID:Logo@x\"></p>\r\n--r\r\n\
        Content-Type: image/png\r\nContent-ID: <logo@x>\r\nContent-Transfer-Encoding: base64\r\n\r\n\
        iVBORw0KGgo=\r\n--r\r\n\
        Content-Type: image/png\r\nContent-ID: <unused@x>\r\nContent-Transfer-Encoding: base64\r\n\r\n\
        iVBORw0KGgo=\r\n--r--\r\n";
    let got = infos(raw);
    assert_eq!(got.len(), 2);
    assert!(got[0].inline);
    assert_eq!(got[0].content_id.as_deref(), Some("logo@x"));
    assert!(!got[1].inline);
}

#[test]
fn a_nested_message_is_one_attachment() {
    let raw = b"From: a@example.com\r\nMIME-Version: 1.0\r\n\
        Content-Type: multipart/mixed; boundary=\"o\"\r\n\r\n--o\r\n\
        Content-Type: text/plain\r\n\r\nSee below.\r\n--o\r\n\
        Content-Type: message/rfc822\r\n\r\n\
        From: b@example.com\r\nSubject: inner\r\nMIME-Version: 1.0\r\n\
        Content-Type: multipart/mixed; boundary=\"i\"\r\n\r\n--i\r\n\
        Content-Type: text/plain\r\n\r\ninner body\r\n--i\r\n\
        Content-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"in.pdf\"\r\n\r\n\
        %PDF-1.4\r\n--i--\r\n--o--\r\n";
    let got = extract(raw).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].info.real_type, "message/rfc822");
    assert_eq!(got[0].info.safe_name, "attachment-1.eml");
    assert!(got[0].data.starts_with(b"From: b@example.com"));
    assert!(!got[0].info.type_mismatch);
}

#[test]
fn a_named_nested_message_is_saved_as_eml() {
    let cases = [
        (Some("Fwd: hello"), "Fwd_ hello.eml"),
        (Some("note.EML"), "note.EML"),
        (Some("scan.pdf"), "scan.pdf.eml"),
    ];
    for (name, want) in cases {
        assert_eq!(safe_name(name, 1, "message/rfc822"), want, "{name:?}");
    }
}
