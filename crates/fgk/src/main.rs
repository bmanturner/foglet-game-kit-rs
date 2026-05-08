//! `fgk` — the foglet-game-kit CLI entry point.
//!
//! Task 1 stands up the binary skeleton only. Subcommand parsing
//! (`new`, `emit-manifest`, `package`) lands in Tasks 10–12, where the
//! relevant `clap` / `anyhow` dependencies are introduced. Until then,
//! invoking `fgk` prints a stub message and exits 0 so smoke tests
//! against the binary succeed.

fn main() {
    // Intentionally minimal: SPEC §15's verification block exercises
    // subcommands that don't exist yet. Printing the crate version
    // gives operators something useful (and gives us a deterministic
    // string for an early CLI smoke test) without committing to a
    // CLI contract before Task 10.
    println!(
        "fgk {} — CLI scaffold (subcommands land in Tasks 10–12)",
        foglet_game::VERSION
    );
}
