//! End-to-end smoke test for `fgk package` (Task 12).
//!
//! Unit tests in `src/package.rs` cover assembly logic, exec-bit
//! handling, and `run.sh` byte equality with SPEC §10.4. This file
//! exercises the wiring from `clap` argument parsing into the
//! `assemble_bundle` path — i.e. the operator-facing CLI path that
//! takes `--binary` and skips the cargo build (running cargo build in
//! a smoke test would dominate the test suite's runtime for no
//! additional invariant beyond what unit tests already cover).
//!
//! The full `cargo build --release` path is exercised manually per
//! SPEC §14, with evidence captured in commit bodies.

use std::fs;

use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn fgk_package_with_binary_writes_full_bundle() {
    let td = tempfile::tempdir().unwrap();
    let project = td.path().join("smoke-game");

    // Scaffold a real project so the package command sees a valid
    // `assets/game.toml` and asset tree.
    Command::cargo_bin("fgk")
        .unwrap()
        .arg("new")
        .arg(&project)
        .assert()
        .success();

    // Stand in for `target/release/<slug>` with a tiny shell script.
    // The packaging pipeline only cares that the path exists and is a
    // file; it never executes the binary.
    let fake_bin = td.path().join("smoke-game-bin");
    fs::write(&fake_bin, b"#!/bin/sh\necho fake\n").unwrap();

    let out = td.path().join("dist");

    Command::cargo_bin("fgk")
        .unwrap()
        .arg("package")
        .arg("--out")
        .arg(&out)
        .arg("--project")
        .arg(&project)
        .arg("--binary")
        .arg(&fake_bin)
        .assert()
        .success()
        .stdout(contains("Packaged `smoke-game`"));

    assert!(out.join("smoke-game").is_file(), "binary missing");
    assert!(out.join("run.sh").is_file(), "run.sh missing");
    assert!(out.join("manifest.json").is_file(), "manifest.json missing");
    assert!(out.join("assets").is_dir(), "assets/ missing");

    // Manifest is valid JSON pointing at the install-dir we defaulted.
    let body = fs::read_to_string(out.join("manifest.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["slug"], "smoke-game");
    assert_eq!(v["command"], "/srv/foglet/doors/smoke-game/run.sh");

    // run.sh references the slug; sanity-check the SPEC §10.4 shape
    // without re-asserting byte equality (covered by unit tests).
    let run_sh = fs::read_to_string(out.join("run.sh")).unwrap();
    assert!(run_sh.contains(r#"exec "$DIR/smoke-game""#));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let bin_mode = fs::metadata(out.join("smoke-game"))
            .unwrap()
            .permissions()
            .mode();
        let run_mode = fs::metadata(out.join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(bin_mode & 0o111, 0o111, "binary missing exec bit");
        assert_eq!(run_mode & 0o111, 0o111, "run.sh missing exec bit");
    }
}

#[test]
fn fgk_package_rejects_relative_install_dir() {
    let td = tempfile::tempdir().unwrap();
    let project = td.path().join("smoke-game");
    Command::cargo_bin("fgk")
        .unwrap()
        .arg("new")
        .arg(&project)
        .assert()
        .success();

    let fake_bin = td.path().join("smoke-game-bin");
    fs::write(&fake_bin, b"#!/bin/sh\n").unwrap();

    let out = td.path().join("dist");

    Command::cargo_bin("fgk")
        .unwrap()
        .arg("package")
        .arg("--out")
        .arg(&out)
        .arg("--project")
        .arg(&project)
        .arg("--binary")
        .arg(&fake_bin)
        .arg("--install-dir")
        .arg("relative/path")
        .assert()
        .failure()
        .stderr(contains("must be an absolute path"));
}
