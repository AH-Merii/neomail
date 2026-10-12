//! install.sh's checks that need no download: the base URL rule, the tools it needs and
//! `--uninstall`. Nothing here reaches the network: curl and wget are stand-ins that only record
//! that they ran.
#![cfg(unix)]
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../install.sh")
}

fn write_exe(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The full path of `tool` on the test's PATH.
fn which(tool: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join(tool))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("{tool} is not on PATH"))
}

/// A bin directory whose curl and wget only touch `ran` in `dir`, so a test can tell whether
/// a download was tried, plus links to `tools`.
fn bin(dir: &Path, tools: &[&str]) -> PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let ran = dir.join("ran");
    for fake in ["curl", "wget"] {
        write_exe(
            &bin.join(fake),
            &format!("touch '{}'; exit 1", ran.display()),
        );
    }
    for tool in tools {
        std::os::unix::fs::symlink(which(tool), bin.join(tool)).unwrap();
    }
    bin
}

fn install(dir: &Path, path: &Path, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut c = Command::new("/bin/sh");
    c.arg(script())
        .args(args)
        .env_clear()
        .env("PATH", path)
        .env("HOME", dir)
        .env("TMPDIR", dir)
        .env("NEOMAIL_INSTALL_DIR", dir.join("dest"));
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn a_base_url_that_is_not_https_is_refused_before_any_download() {
    for url in [
        "http://mirror.example/edge",
        "ftp://mirror.example/edge",
        "http://127.0.0.1.mirror.example/edge",
        "http://localhost:80@mirror.example/edge",
        "http://127.0.0.1@mirror.example",
        "http://localhost:x@mirror.example/edge",
        "http://localhost:/edge",
        "https://",
        "mirror.example/edge",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let bin = bin(dir.path(), &[]);
        let path = std::env::join_paths([bin, which("uname").parent().unwrap().into()]).unwrap();
        let o = install(
            dir.path(),
            Path::new(&path),
            &[("NEOMAIL_INSTALL_BASE_URL", url)],
            &[],
        );
        assert_eq!(o.status.code(), Some(1), "{url}: {}", stderr(&o));
        assert!(
            stderr(&o).contains("NEOMAIL_INSTALL_BASE_URL must be https"),
            "{url}: {}",
            stderr(&o)
        );
        assert!(!dir.path().join("ran").exists(), "{url}: a download ran");
    }
}

// Linux only: macOS has shasum, not sha256sum, and the link list below names sha256sum.
#[cfg(target_os = "linux")]
#[test]
fn a_missing_awk_is_named_before_any_download() {
    // http on 127.0.0.1 or localhost passes the URL rule; awk is checked next to tar and mktemp.
    for url in [
        "http://127.0.0.1:1/edge",
        "http://localhost:1",
        "https://mirror.example",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let tools = ["uname", "sha256sum", "tar", "mktemp"];
        let bin = bin(dir.path(), &tools);
        let o = install(dir.path(), &bin, &[("NEOMAIL_INSTALL_BASE_URL", url)], &[]);
        assert_eq!(o.status.code(), Some(1), "{url}: {}", stderr(&o));
        assert!(
            stderr(&o).contains("awk is not installed"),
            "{url}: {}",
            stderr(&o)
        );
        assert!(!dir.path().join("ran").exists(), "{url}: a download ran");
    }
}

#[test]
fn uninstall_gives_the_keyring_step_first_and_names_only_existing_directories() {
    for existing in [&[][..], &["config"][..], &["config", "data"][..]] {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest");
        std::fs::create_dir(&dest).unwrap();
        write_exe(&dest.join("neomail"), "echo 'neomail 0.1.0'");
        for d in existing {
            std::fs::create_dir_all(dir.path().join(d).join("neomail")).unwrap();
        }
        let config = dir.path().join("config");
        let data = dir.path().join("data");
        let o = install(
            dir.path(),
            Path::new(&std::env::var_os("PATH").unwrap()),
            &[
                ("XDG_CONFIG_HOME", config.to_str().unwrap()),
                ("XDG_DATA_HOME", data.to_str().unwrap()),
            ],
            &["--uninstall"],
        );
        let err = stderr(&o);
        assert!(o.status.success(), "{err}");
        assert!(!dest.join("neomail").exists());
        let keyring = err.find("neomail account logout NAME").unwrap();
        let removed = err.find("Removed ").unwrap();
        assert!(keyring < removed, "the keyring step comes first:\n{err}");
        assert_eq!(err.contains("rm -r"), !existing.is_empty(), "{err}");
        for (d, path) in [("config", &config), ("data", &data)] {
            let named = err.contains(&format!("\"{}\"", path.join("neomail").display()));
            assert_eq!(named, existing.contains(&d), "{d}:\n{err}");
        }
    }
}
