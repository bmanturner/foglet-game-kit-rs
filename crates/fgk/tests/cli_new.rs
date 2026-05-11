//! End-to-end smoke test for `fgk new <path>`.
//!
//! Unit tests in `src/scaffold.rs` cover the generation logic in
//! detail; this file proves the wiring from `clap` argument parsing
//! through to the scaffolder is intact when `fgk` is driven as a
//! real binary. Running `cargo test` inside the generated project is
//! manual evidence only — compiling a fresh crate inside the
//! workspace's own `cargo test` run is too slow for the inner loop.

use assert_cmd::Command;
use predicates::str::contains;
use std::process::Command as ProcessCommand;

#[test]
fn fgk_new_creates_project_files_at_destination() {
    let td = tempfile::tempdir().unwrap();
    let dest = td.path().join("smoke-game");

    Command::cargo_bin("fgk")
        .expect("fgk binary should be built by cargo test")
        .arg("new")
        .arg(&dest)
        .assert()
        .success()
        .stdout(contains("Created new Foglet game project"));

    // Spot-check required outputs. The exhaustive template-presence
    // check lives in `scaffold::tests`; this just proves the binary
    // actually wrote files at the requested path.
    for rel in [
        "Cargo.toml",
        "src/main.rs",
        "assets/game.toml",
        ".gitignore",
        "README.md",
    ] {
        let p = dest.join(rel);
        assert!(p.is_file(), "expected `{}` to exist", p.display());
    }
}

#[test]
fn fgk_new_rejects_invalid_slug_in_final_component() {
    let td = tempfile::tempdir().unwrap();
    let dest = td.path().join("Bad_Name");

    Command::cargo_bin("fgk")
        .unwrap()
        .arg("new")
        .arg(&dest)
        .assert()
        .failure()
        .stderr(contains("not a valid slug"));

    // No partial directory must be left behind after a validation
    // failure. The unit test asserts the same property; this
    // re-asserts it through the binary surface.
    assert!(
        !dest.exists(),
        "scaffolder must not create the dest dir on validation failure"
    );
}

#[test]
fn fgk_new_scaffold_passes_cargo_test_in_generated_project() {
    let td = tempfile::tempdir().unwrap();
    let dest = td.path().join("scaffold-testable-game");

    Command::cargo_bin("fgk")
        .expect("fgk binary should be built by cargo test")
        .arg("new")
        .arg(&dest)
        .assert()
        .success();

    let status = ProcessCommand::new("cargo")
        .arg("test")
        .arg("--quiet")
        .current_dir(&dest)
        .status()
        .expect("should be able to run cargo test in scaffolded project");

    assert!(
        status.success(),
        "cargo test in scaffolded project should pass (status: {status})"
    );
}
