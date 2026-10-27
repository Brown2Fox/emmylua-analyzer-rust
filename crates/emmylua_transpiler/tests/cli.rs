use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "emmylua-transpiler-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_emmylua_compile"));
        command.current_dir(&self.0);
        command
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn emits_checked_lua_using_external_annotations() {
    let fixture = Fixture::new();
    let library = Fixture::new();
    let sdk = library.0.join("sdk.d.lua");
    fs::write(&sdk, "---@class Camera\n---@field zoom number\nlocal Camera = {}\n---@return Camera\nfunction getCamera() end\n").unwrap();
    let source = "local camera: Camera = getCamera()\nreturn camera.zoom\n";
    fs::write(fixture.0.join("camera.cua"), source).unwrap();
    let result = fixture
        .command()
        .args(["camera.cua", "-o", "camera.lua", "-L"])
        .arg(&sdk)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output = fs::read_to_string(fixture.0.join("camera.lua")).unwrap();
    assert_eq!(output.len(), source.len());
    assert!(!output.contains(": Camera"));
    assert!(output.contains("return camera.zoom"));
}

#[test]
fn failed_check_preserves_output_and_reports_original_unicode_location() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("bad.cua"), "local function f(x: number): number return x end\nlocal text = 'привет🙂'; f('wrong')\nreturn text\n").unwrap();
    fs::write(fixture.0.join("out.lua"), "previous output").unwrap();
    let result = fixture
        .command()
        .args(["bad.cua", "-o", "out.lua"])
        .output()
        .unwrap();
    assert!(
        !result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains(":2:27:"), "{stderr}");
    assert_eq!(
        fs::read_to_string(fixture.0.join("out.lua")).unwrap(),
        "previous output"
    );
}

#[test]
fn errors_do_not_create_output() {
    let fixture = Fixture::new();
    for source in [
        "local n: = 1",
        "local function f(x: unknown): number return x end",
        "local function f(x) return x end",
        "local x: MissingType = {}",
        "---@meta\nlocal n: number = 'bad'",
    ] {
        fs::write(fixture.0.join("bad.cua"), source).unwrap();
        let result = fixture
            .command()
            .args(["bad.cua", "-o", "out.lua"])
            .output()
            .unwrap();
        assert!(!result.status.success(), "{source}");
        assert!(!fixture.0.join("out.lua").exists());
    }
}

#[test]
fn check_only_and_source_overwrite_protection() {
    let fixture = Fixture::new();
    let source = "local n: number = 1\nreturn n\n";
    fs::write(fixture.0.join("input.cua"), source).unwrap();
    let checked = fixture
        .command()
        .args(["input.cua", "--check"])
        .output()
        .unwrap();
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let overwrite = fixture
        .command()
        .args(["input.cua", "-o", "input.cua"])
        .output()
        .unwrap();
    assert!(!overwrite.status.success());
    assert_eq!(
        fs::read_to_string(fixture.0.join("input.cua")).unwrap(),
        source
    );
}

#[test]
fn workspace_loads_cua_modules_and_configured_libraries() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("types")).unwrap();
    fs::write(fixture.0.join(".emmyrc.json"), r#"{"workspace":{"library":["./types"]},"strict":{"noImplicitAny":true,"checkUnknown":true}}"#).unwrap();
    fs::write(fixture.0.join("types/sdk.lua"), "---@meta\n---@class Camera\n---@field zoom number\nlocal Camera = {}\n---@return Camera\nfunction getCamera() end\n").unwrap();
    fs::write(
        fixture.0.join("camera.cua"),
        "local M = {}\nfunction M.zoom(camera: Camera): number return camera.zoom end\nreturn M\n",
    )
    .unwrap();
    fs::write(
        fixture.0.join("main.cua"),
        "local camera = require('camera')\nlocal value: number = camera.zoom(getCamera())\nreturn value\n",
    )
    .unwrap();
    let result = fixture
        .command()
        .args(["main.cua", "--workspace", ".", "-o", "out.lua"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    fs::write(
        fixture.0.join("main.cua"),
        "local camera = require('camera')\nreturn camera.zoom('invalid')\n",
    )
    .unwrap();
    let result = fixture
        .command()
        .args(["main.cua", "--workspace", ".", "--check"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("Camera"), "{stderr}");
}
