#![allow(clippy::unwrap_used)]

use std::time::{Duration, SystemTime};

use super::open::{confirm, opener, remove_old};
use super::save::write_file;
use crate::secret::Input;

#[test]
fn the_configured_opener_wins_then_the_platform_default() {
    let none = |_: &str| false;
    assert_eq!(
        opener(Some("stub --flag"), true, true, |_| true),
        ["stub", "--flag"]
    );
    assert_eq!(opener(Some("  "), false, false, none), ["xdg-open"]);
    assert_eq!(opener(None, true, false, none), ["open"]);
    assert_eq!(opener(None, false, true, |p| p == "wslview"), ["wslview"]);
    assert_eq!(opener(None, false, true, none), ["xdg-open"]);
}

#[test]
fn saving_never_replaces_a_file_unless_asked() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "mine").unwrap();
    let err = write_file(&path, b"theirs", false).unwrap_err();
    assert!(format!("{err:#}").contains("--overwrite"), "{err:#}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine");
    write_file(&path, b"theirs", true).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "theirs");
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "no temporary file left"
    );
}

#[cfg(unix)]
#[test]
fn saving_never_writes_through_a_symbolic_link() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    std::fs::write(&target, "kept").unwrap();
    let link = dir.path().join("a.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(write_file(&link, b"x", false).is_err());
    write_file(&link, b"x", true).unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "kept");
    assert!(!std::fs::symlink_metadata(&link).unwrap().is_symlink());
}

#[cfg(unix)]
#[test]
fn saved_files_are_private_and_not_executable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    for overwrite in [false, true] {
        let path = dir.path().join(format!("f{overwrite}"));
        write_file(&path, b"#!/bin/sh\n", overwrite).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{overwrite}");
    }
}

#[test]
fn copies_older_than_a_day_are_removed() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("42-1")).unwrap();
    remove_old(root.path(), SystemTime::now());
    assert!(root.path().join("42-1").exists());
    remove_old(
        root.path(),
        SystemTime::now() + Duration::from_secs(25 * 60 * 60),
    );
    assert!(!root.path().join("42-1").exists());
}

#[test]
fn a_copy_edited_in_place_is_kept_a_day_from_the_edit() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("42-1");
    std::fs::create_dir(&dir).unwrap();
    let now = SystemTime::now();
    let file = std::fs::File::create(dir.join("form.pdf")).unwrap();
    file.set_modified(now + Duration::from_secs(20 * 60 * 60))
        .unwrap();
    remove_old(root.path(), now + Duration::from_secs(25 * 60 * 60));
    assert!(dir.exists());
    remove_old(root.path(), now + Duration::from_secs(45 * 60 * 60));
    assert!(!dir.exists());
}

fn executable() -> nm_core::attachment::Info {
    let raw = b"From: a@example.com\r\nMIME-Version: 1.0\r\n\
        Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\n\
        Content-Type: text/plain\r\n\r\nbody\r\n--b\r\n\
        Content-Type: application/octet-stream\r\n\
        Content-Disposition: attachment; filename=\"run.sh\"\r\n\r\n\
        #!/bin/sh\r\n--b--\r\n";
    let info = nm_core::attachment::extract(raw).unwrap().remove(0).info;
    assert!(info.executable);
    info
}

#[test]
fn an_executable_opens_on_a_terminal_only_after_a_yes() {
    let terminal = Input {
        terminal: true,
        no_input: false,
    };
    let info = executable();
    for (answer, opens) in [
        ("n", false),
        ("", false),
        ("no", false),
        ("y", true),
        ("YES\n", true),
    ] {
        let mut asked = false;
        let got = confirm(&info, false, terminal, |_| {
            asked = true;
            Ok(answer.to_owned())
        })
        .unwrap();
        assert!(asked, "{answer:?}");
        assert_eq!(got, opens, "{answer:?}");
    }
    let got = confirm(&info, true, terminal, |_| -> anyhow::Result<String> {
        panic!("--yes must not ask")
    });
    assert!(got.unwrap());
}

#[test]
fn an_executable_is_refused_off_a_terminal_without_asking() {
    for input in [
        Input {
            terminal: false,
            no_input: false,
        },
        Input {
            terminal: true,
            no_input: true,
        },
    ] {
        let got = confirm(&executable(), false, input, |_| -> anyhow::Result<String> {
            panic!("off a terminal nothing is asked")
        });
        assert!(format!("{:#}", got.unwrap_err()).contains("--yes"));
    }
}

#[test]
fn a_terminal_listing_says_each_note_in_words_and_the_declared_type_when_it_differs() {
    let raw = b"From: a@example.com\r\nMIME-Version: 1.0\r\n\
        Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\n\
        Content-Type: text/plain\r\n\r\nbody\r\n--b\r\n\
        Content-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"a.pdf\"\r\n\r\n\
        %PDF-1.4\r\n--b\r\n\
        Content-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"b.pdf\"\r\n\r\n\
        #!/bin/sh\r\n--b--\r\n";
    let attachments = nm_core::attachment::extract(raw).unwrap();
    let mut out = Vec::new();
    super::write_human(&mut out, &attachments, crate::style::Paint::OFF).unwrap();
    let out = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert!(lines[0].starts_with("  1  a.pdf  application/pdf"), "{out}");
    assert!(
        !lines[0].contains("declared") && !lines[0].contains("mismatch"),
        "{out}"
    );
    assert!(lines[1].starts_with("  2  b.pdf  text/x-script"), "{out}");
    assert!(lines[1].contains("(declared application/pdf)"), "{out}");
    assert!(lines[1].ends_with("mismatch, executable"), "{out}");
}
