//! End-to-end smoke test for `fgk emit-manifest` (Task 11).
//!
//! Unit tests in `src/emit_manifest.rs` cover the assembly logic in
//! detail; this file proves the wiring from `clap` argument parsing
//! through stdout is intact when `fgk` runs as a real binary. We
//! exercise the success path against a `fgk new`-scaffolded project
//! and the failure path against a relative `--install-dir`.

use assert_cmd::Command;
use predicates::str::contains;

/// Scaffold a fresh project with `fgk new`, then point
/// `fgk emit-manifest` at it. The scaffolded project ships a SPEC §9.1
/// `assets/game.toml`, so the emitted JSON is the canonical SPEC §10.3
/// example modulo the slug derived from the temp path.
#[test]
fn fgk_emit_manifest_prints_spec_10_3_shape_to_stdout() {
    let td = tempfile::tempdir().unwrap();
    let dest = td.path().join("smoke-game");

    Command::cargo_bin("fgk")
        .unwrap()
        .arg("new")
        .arg(&dest)
        .assert()
        .success();

    let install_dir = "/srv/foglet/doors/smoke-game";
    let output = Command::cargo_bin("fgk")
        .unwrap()
        .arg("emit-manifest")
        .arg("--install-dir")
        .arg(install_dir)
        .arg("--project")
        .arg(&dest)
        .assert()
        .success()
        .get_output()
        .clone();

    let json = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert!(
        json.ends_with('\n'),
        "emit-manifest should leave a trailing newline so file redirects look normal"
    );
    let v: serde_json::Value = serde_json::from_str(&json).expect("emitted JSON parses");
    assert_eq!(v["runtime"], "external_pty");
    assert_eq!(v["command"], "/srv/foglet/doors/smoke-game/run.sh");
    assert_eq!(v["working_dir"], install_dir);
    assert_eq!(v["pty"], true);
    assert_eq!(v["slug"], "smoke-game");
}

#[test]
fn fgk_emit_manifest_rejects_relative_install_dir() {
    let td = tempfile::tempdir().unwrap();
    let dest = td.path().join("smoke-game");
    Command::cargo_bin("fgk")
        .unwrap()
        .arg("new")
        .arg(&dest)
        .assert()
        .success();

    Command::cargo_bin("fgk")
        .unwrap()
        .arg("emit-manifest")
        .arg("--install-dir")
        .arg("relative/path")
        .arg("--project")
        .arg(&dest)
        .assert()
        .failure()
        .stderr(contains("must be an absolute path"));
}
