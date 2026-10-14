//! `neomail attachment list|save|open` against a temporary cache of synthetic messages. No
//! network, and no real opener: `NEOMAIL_OPENER` is a script that records what it was given.
#![cfg(unix)]
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use nm_core::{Cache, FetchedMessage, Flags, Uid, UidValidity};
use serde_json::{Value, json};

const PDF: &str = "JVBERi0xLjcKJeLjz9MK"; // "%PDF-1.7\n%âãÏÓ\n"
const PNG: &str = "iVBORw0KGgoAAAANSUhEUg=="; // the PNG signature and an IHDR header
const ELF: &str = "f0VMRgIBAQAAAAAA"; // "\x7fELF\x02\x01\x01" and padding

fn part(ct: &str, name: &str, base64: &str) -> String {
    format!(
        "--b\r\nContent-Type: {ct}\r\nContent-Disposition: attachment; filename=\"{name}\"\r\n\
         Content-Transfer-Encoding: base64\r\n\r\n{base64}\r\n"
    )
}

/// Message 1 has a PDF, a PNG declared as a PDF, an ELF named like a photo and a name that tries
/// to leave the directory; message 2 has no attachments.
fn seeded() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut raw = "From: a@example.com\r\nSubject: files\r\nMIME-Version: 1.0\r\n\
        Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\n\
        Here.\r\n"
        .to_owned();
    raw += &part("application/pdf", "report.pdf", PDF);
    raw += &part("application/pdf", "chart.pdf", PNG);
    raw += &part("image/jpeg", "photo.jpg", ELF);
    raw += &part("text/plain", "../../escape\u{202E}txt.sh", "aGk=");
    raw += "--b--\r\n";
    let plain = "From: a@example.com\r\nSubject: none\r\n\r\nNo files.\r\n";
    let fetched = |uid: u32, raw: &str| FetchedMessage {
        uid: Uid::new(uid).unwrap(),
        flags: Flags::new(Vec::<String>::new()),
        received: None,
        raw: raw.as_bytes().to_vec(),
    };
    let mut cache = Cache::open(&dir.path().join("cache.sqlite3")).unwrap();
    let mailbox = cache
        .reset_mailbox("work", "INBOX", UidValidity::new(1).unwrap())
        .unwrap();
    cache
        .insert_messages(mailbox.id, &[fetched(1, &raw), fetched(2, plain)])
        .unwrap();
    dir
}

/// The binary with its data in `data`, standard input not a terminal, and the opener a script
/// that appends each argument it gets to `data/opened`.
fn neomail(data: &Path, cwd: &Path, args: &[&str]) -> Output {
    let stub = data.join("opener.sh");
    if !stub.exists() {
        let log = data.join("opened");
        std::fs::write(
            &stub,
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut c = Command::new(env!("CARGO_BIN_EXE_neomail"));
    c.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .env("NEOMAIL_DATA_DIR", data)
        .env("NEOMAIL_CONFIG", data.join("config.toml"))
        .env("HOME", data)
        .env("NEOMAIL_OPENER", &stub)
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent");
    for var in [
        "XDG_DATA_HOME",
        "NEOMAIL_NO_INPUT",
        "CLICOLOR_FORCE",
        "NO_COLOR",
    ] {
        c.env_remove(var);
    }
    c.output().unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn opened(data: &Path) -> Option<String> {
    std::fs::read_to_string(data.join("opened")).ok()
}

#[test]
fn list_json_gives_each_attachment_its_index_types_size_and_warnings() {
    let data = seeded();
    let o = neomail(
        data.path(),
        data.path(),
        &["attachment", "list", "1", "--json"],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(
        v[0],
        json!({
            "id": 1, "account": "work", "index": 1, "name": "report.pdf",
            "safe_name": "report.pdf", "declared_type": "application/pdf",
            "real_type": "application/pdf", "type_mismatch": false, "size": 15,
            "inline": false, "content_id": null, "executable": false
        })
    );
    let field = |i: usize, f: &str| v[i][f].clone();
    assert_eq!(field(1, "real_type"), "image/png");
    assert_eq!(field(1, "type_mismatch"), true);
    assert_eq!(field(2, "type_mismatch"), true);
    assert_eq!(field(2, "executable"), true);
    assert_eq!(field(3, "name"), "../../escape\u{202E}txt.sh");
    assert_eq!(field(3, "safe_name"), "escapetxt.sh");
    assert_eq!(field(3, "executable"), true);
}

#[test]
fn list_off_a_terminal_is_tsv_and_a_message_without_attachments_lists_nothing() {
    let data = seeded();
    let o = neomail(data.path(), data.path(), &["attachment", "list", "1"]);
    let out = String::from_utf8(o.stdout).unwrap();
    let rows: Vec<Vec<&str>> = out.lines().map(|l| l.split('\t').collect()).collect();
    assert_eq!(rows.len(), 4, "{out}");
    assert_eq!(
        rows[1],
        [
            "2",
            "chart.pdf",
            "image/png",
            "application/pdf",
            "16",
            "mismatch"
        ]
    );
    let o = neomail(
        data.path(),
        data.path(),
        &["attachment", "list", "2", "--json"],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(
        serde_json::from_slice::<Value>(&o.stdout).unwrap(),
        json!([])
    );
}

#[test]
fn an_unknown_message_or_index_exits_1() {
    let data = seeded();
    let o = neomail(data.path(), data.path(), &["attachment", "list", "99"]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    let o = neomail(data.path(), data.path(), &["attachment", "open", "1", "9"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains("has no attachment 9; it has 1 to 4"),
        "{}",
        stderr(&o)
    );
    assert_eq!(opened(data.path()), None);
}

#[test]
fn save_writes_every_attachment_into_the_current_directory_under_safe_names() {
    let data = seeded();
    let cwd = tempfile::tempdir().unwrap();
    let o = neomail(
        data.path(),
        cwd.path(),
        &["attachment", "save", "1", "--json"],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    let paths: Vec<PathBuf> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|s| PathBuf::from(s["path"].as_str().unwrap()))
        .collect();
    let names = ["report.pdf", "chart.pdf", "photo.jpg", "escapetxt.sh"];
    let mut listed: Vec<_> = std::fs::read_dir(cwd.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    listed.sort();
    let mut expected: Vec<_> = names.iter().map(|n| cwd.path().join(n)).collect();
    expected.sort();
    assert_eq!(listed, expected, "nothing written outside the directory");
    // neomail prints where each file went with the directory resolved: on macOS the temporary
    // directory is under /var, a link to /private/var.
    let dir = cwd.path().canonicalize().unwrap();
    assert_eq!(paths, names.map(|n| dir.join(n)));
    assert_eq!(
        std::fs::read(&paths[0]).unwrap(),
        b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n"
    );
    assert_eq!(std::fs::read_to_string(&paths[3]).unwrap(), "hi");
}

#[test]
fn save_keeps_an_existing_file_unless_told_to_overwrite() {
    let data = seeded();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("report.pdf"), "mine").unwrap();
    let to = dir.path().to_str().unwrap();
    let o = neomail(
        data.path(),
        data.path(),
        &["attachment", "save", "1", "1", "2", "--dir", to],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("--overwrite"), "{}", stderr(&o));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("report.pdf")).unwrap(),
        "mine"
    );
    let out = String::from_utf8(o.stdout).unwrap();
    assert_eq!(
        out,
        format!("2\t{}\n", dir.path().join("chart.pdf").display())
    );
    let o = neomail(
        data.path(),
        data.path(),
        &["attachment", "save", "1", "1", "--dir", to, "--overwrite"],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        std::fs::read(dir.path().join("report.pdf"))
            .unwrap()
            .starts_with(b"%PDF")
    );
}

#[test]
fn open_writes_a_private_copy_and_hands_it_to_the_opener() {
    let data = seeded();
    let o = neomail(
        data.path(),
        data.path(),
        &["attachment", "open", "1", "1", "--json"],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    let path = PathBuf::from(v["path"].as_str().unwrap());
    assert_eq!(
        opened(data.path()).unwrap(),
        format!("{}\n", path.display())
    );
    assert!(
        path.starts_with(data.path().join("attachments")),
        "{}",
        path.display()
    );
    assert!(std::fs::read(&path).unwrap().starts_with(b"%PDF"));
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    assert_eq!(mode(&data.path().join("attachments")), 0o700);
}

#[test]
fn open_refuses_an_executable_without_a_terminal_unless_told_yes() {
    let data = seeded();
    for index in ["3", "4"] {
        let o = neomail(
            data.path(),
            data.path(),
            &["attachment", "open", "1", index],
        );
        assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
        assert!(
            stderr(&o).contains("executable or a script"),
            "{}",
            stderr(&o)
        );
        assert_eq!(opened(data.path()), None, "nothing opened");
    }
    let o = neomail(
        data.path(),
        data.path(),
        &["attachment", "open", "1", "3", "--yes"],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(opened(data.path()).unwrap().ends_with("photo.jpg\n"));
}
