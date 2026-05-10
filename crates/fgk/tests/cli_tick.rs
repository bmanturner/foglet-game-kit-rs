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
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::time::Duration;

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

/// Directory-backed SQL fixture used by Task 13d completion checks.
///
/// Keeping these files on disk (instead of hard-coding SQL in this
/// test) gives the Ralph loop a literal migration directory to list
/// when verifying Completion condition 7.
const V4_WORLD_MIGRATIONS_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/v4-world-migrations"
);

/// Return sorted `.sql` migration files from `dir`.
///
/// Sorting by filename keeps application order stable across filesystems
/// and matches the numeric prefixes used in the fixture.
fn sorted_sql_migration_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("read migration directory `{}`: {error}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|error| {
                    panic!("read migration entry in `{}`: {error}", dir.display())
                })
                .path()
        })
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("sql"))
        .collect();
    files.sort();
    files
}

/// Apply every SQL file from `migration_dir` to `world`.
///
/// This intentionally uses the directory fixture directly so the test
/// exercises the same "list migration files, apply each to a fresh DB"
/// path that Completion condition 7 asks for.
fn apply_migration_directory(world: &WorldDb, migration_dir: &Path) {
    for sql_path in sorted_sql_migration_files(migration_dir) {
        let sql = fs::read_to_string(&sql_path)
            .unwrap_or_else(|error| panic!("read `{}`: {error}", sql_path.display()));
        world
            .connection()
            .execute_batch(&sql)
            .unwrap_or_else(|error| panic!("apply `{}`: {error}", sql_path.display()));
    }
}

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

#[test]
fn fgk_tick_can_be_interrupted_during_a_callback_without_advancing_last_run_at() {
    let td = tempfile::tempdir().expect("tempdir");
    let project = td.path().join("interruption-project");
    fs::create_dir_all(&project).expect("create project dir");
    write_fixture_project(&project);

    let world_path = project.join("world").join("world.sqlite");
    let mut world = WorldDb::open(&world_path).expect("seed world db");
    world
        .apply_migration(&WORLD_TICK_TASKS_MIGRATION)
        .expect("world tick migration exists");

    world
        .register_tick("interruption_tick", 600, |_tx| Ok(()))
        .expect("seed interruption task");

    let mut command = StdCommand::new(env!("CARGO_BIN_EXE_fgk"));
    command
        .arg("tick")
        .arg("--project")
        .arg(&project)
        .env("FGK_TICK_TEST_CALLBACK_DELAY_MS", "2000");

    let mut child = command.spawn().expect("spawn fgk tick command");
    std::thread::sleep(Duration::from_millis(250));
    assert!(
        child
            .try_wait()
            .expect("checking fgk tick child status")
            .is_none(),
        "fgk tick should still be running while interruption test waits"
    );

    #[cfg(unix)]
    {
        let signal = std::process::Command::new("kill")
            .arg("-INT")
            .arg(child.id().to_string())
            .status()
            .expect("send Ctrl-C to fgk tick");
        assert!(signal.success(), "SIGINT dispatch should work");
    }

    #[cfg(not(unix))]
    {
        child.kill().expect("terminate fgk tick command");
    }

    let output = child.wait_with_output().expect("wait for interrupted tick");
    assert!(
        !output.status.success(),
        "interrupted command should not succeed"
    );

    let last_run: Option<String> = world
        .connection()
        .query_row(
            "SELECT last_run_at FROM world_tick_tasks WHERE key = ?1",
            ["interruption_tick"],
            |row| row.get(0),
        )
        .expect("query last_run_at");
    assert!(
        last_run.is_none(),
        "interrupted callback must not advance last_run_at"
    );
}

#[test]
fn v4_migration_directory_initializes_all_required_tables_in_fresh_world_db() {
    let migration_dir = Path::new(V4_WORLD_MIGRATIONS_DIR);
    assert!(
        migration_dir.is_dir(),
        "v4 migration fixture directory must exist at `{}`",
        migration_dir.display()
    );

    let files = sorted_sql_migration_files(migration_dir);
    for required in [
        "0010_create_places.sql",
        "0011_create_routes.sql",
        "0012_create_presence.sql",
        "0013_create_place_recall.sql",
        "0014_create_inventory_slots.sql",
        "0015_create_world_tick_tasks.sql",
    ] {
        assert!(
            files
                .iter()
                .any(|path| { path.file_name().and_then(|name| name.to_str()) == Some(required) }),
            "migration fixture should contain `{required}`"
        );
    }

    let td = tempfile::tempdir().expect("tempdir");
    let world_path = td.path().join("fresh-world.sqlite");
    let world = WorldDb::open(&world_path).expect("open fresh world db");

    let pre_apply_count: i64 = world
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' \
             AND name IN ('places','routes','presence','place_recall','inventory_slots','world_tick_tasks')",
            [],
            |row| row.get(0),
        )
        .expect("count tables before applying v4 fixture");
    assert_eq!(
        pre_apply_count, 0,
        "fresh world should not have v4 tables before applying fixture migrations"
    );

    apply_migration_directory(&world, migration_dir);

    let mut stmt = world
        .connection()
        .prepare(
            "SELECT name FROM sqlite_master \
             WHERE type = 'table' \
             AND name IN ('places','routes','presence','place_recall','inventory_slots','world_tick_tasks') \
             ORDER BY name",
        )
        .expect("prepare table-list query");
    let names: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .expect("query table names")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect table names");

    assert_eq!(
        names,
        vec![
            "inventory_slots".to_string(),
            "place_recall".to_string(),
            "places".to_string(),
            "presence".to_string(),
            "routes".to_string(),
            "world_tick_tasks".to_string()
        ],
        "all required v4 tables should exist after applying migration directory fixture"
    );
}
