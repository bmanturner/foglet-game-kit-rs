//! End-to-end smoke test for `fgk tick --project <path>` (Task 10a).
//!
//! This test uses a local synthetic project + world DB fixture to prove
//! that:
//! - the CLI subcommand resolves project config,
//! - the command opens `world/world.sqlite`,
//! - and `run_due_ticks(now)` executes all due rows (for this one-shot
//!   invocation, no concurrency or catch-up limit is involved).
//!
//! The fixture is intentionally genre-neutral:
//! - A **space exploration** door can interpret this as station upkeep;
//! - A **dungeon crawler** door can interpret it as patrol reset.

use std::fs;

use assert_cmd::Command;
use foglet_game::{WorldDb, WORLD_TICK_TASKS_MIGRATION};
use predicates::str::contains;

const GAME_TOML_V4_TICK_FIXTURE: &str = r#"
[game]
title = "Tick Test"
slug = "tick-smoke"
description = "A world-tick smoke fixture."
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[world]
enabled = true
path = "world/world.sqlite"

[world_ticks]
enabled = true
max_catchup_per_call = 1
"#;

/// Create a minimal, v4-friendly project fixture with `assets/game.toml`.
fn write_fixture_project(project: &std::path::Path) {
    let assets = project.join("assets");
    fs::create_dir_all(&assets).expect("assets dir exists");
    fs::write(assets.join("game.toml"), GAME_TOML_V4_TICK_FIXTURE).expect("write game config");
}

#[test]
fn fgk_tick_executes_due_world_tasks_once() {
    let td = tempfile::tempdir().expect("tempdir");
    let project = td.path().join("tick-project");
    fs::create_dir_all(&project).expect("create project dir");
    write_fixture_project(&project);

    // Seed the project's world DB with tick rows exactly as runtime would.
    // `fgk tick` must map each row to an in-process callback before
    // calling `run_due_ticks`.
    let world_path = project.join("world").join("world.sqlite");
    let mut world = WorldDb::open(&world_path).expect("seed world db");
    world
        .apply_migration(&WORLD_TICK_TASKS_MIGRATION)
        .expect("world tick migration exists");

    world
        .register_tick("station_restock", 600, |_tx| Ok(()))
        .expect("seed station task");

    world
        .register_tick("patrol_restart", 600, |_tx| Ok(()))
        .expect("seed patrol task");

    Command::cargo_bin("fgk")
        .unwrap()
        .arg("tick")
        .arg("--project")
        .arg(&project)
        .assert()
        .success()
        .stdout(contains("Ran 1 task(s), skipped 1 task(s)"));

    let rows_due: i64 = world
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM world_tick_tasks WHERE last_run_at IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .expect("count query");
    assert_eq!(rows_due, 1, "catch-up bound should leave one task pending");
}

#[test]
fn fgk_tick_reports_missing_world_db_and_does_not_create_file() {
    let td = tempfile::tempdir().expect("tempdir");
    let project = td.path().join("missing-world");
    fs::create_dir_all(&project).expect("create project dir");
    write_fixture_project(&project);

    let world_path = project.join("world").join("world.sqlite");
    assert!(
        !world_path.exists(),
        "fixture should start with no world db"
    );

    Command::cargo_bin("fgk")
        .unwrap()
        .arg("tick")
        .arg("--project")
        .arg(&project)
        .assert()
        .failure()
        .stderr(contains("world db is missing at"))
        .stderr(contains(world_path.to_string_lossy()));

    assert!(
        !world_path.exists(),
        "failure should not create missing world db"
    );
    assert!(
        !project.join("world").exists(),
        "failure should not create world directory"
    );
}
