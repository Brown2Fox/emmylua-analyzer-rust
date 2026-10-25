#![cfg(feature = "cli")]

use std::{
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Workspace(PathBuf);
impl Workspace {
    fn new(source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "emmylua-gcc-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("main.lua"), source).unwrap();
        Self(path)
    }
    fn check(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_emmylua_check"))
            .arg(&self.0)
            .args(["-f", "gcc"])
            .args(args)
            .output()
            .unwrap()
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn clean_workspace_is_silent() {
    let workspace = Workspace::new("return 1");
    let output = workspace.check(&[]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty(), "{:?}", output);
    assert!(output.stderr.is_empty(), "{:?}", output);
}

#[test]
fn warnings_are_one_line_and_exit_status_is_preserved() {
    let workspace = Workspace::new("---@type string\nlocal value = 1\nreturn value\n");
    for warnings_as_errors in [false, true] {
        let args = if warnings_as_errors {
            vec!["--severity", "warn", "--warnings-as-errors"]
        } else {
            vec!["--severity", "warn"]
        };
        let output = workspace.check(&args);
        assert_eq!(
            output.status.code(),
            Some(if warnings_as_errors { 1 } else { 0 })
        );
        assert!(output.stderr.is_empty(), "{:?}", output);
        let text = String::from_utf8(output.stdout).unwrap();
        assert_eq!(text.lines().count(), 1, "{text}");
        let expected = workspace
            .0
            .join("main.lua")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let expected = expected.strip_prefix("//?/").unwrap_or(&expected);
        assert!(text.starts_with(&format!("{expected}:2:")), "{text}");
        assert!(text.contains(": warning: "), "{text}");
        assert!(!text.contains('\x1b'));
    }
    let filtered = workspace.check(&["--severity", "error"]);
    assert!(filtered.status.success());
    assert!(filtered.stdout.is_empty());
    assert!(filtered.stderr.is_empty());
}

#[test]
fn errors_have_no_extra_failure_banner() {
    let workspace = Workspace::new("local =");
    let output = workspace.check(&["--severity", "error"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty(), "{:?}", output);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.is_empty());
    assert!(
        text.lines()
            .all(|line| line.contains("main.lua:") && line.contains(": error: ")),
        "{text}"
    );
}
