//! Embedded `fgk new` project templates.
//!
//! Task 10a stands up the **fixtures** that `fgk new` (Task 10b) will
//! materialise on disk. Each [`Template`] carries the destination path
//! relative to the new project's root and the file contents, with
//! `{name}` placeholders left in place for [`substitute`] to fill in
//! at generation time.
//!
//! ## Why `include_str!` instead of a build script
//!
//! The templates are plain text fixtures the loop edits often during
//! the early days of the kit. Reading them through `include_str!` keeps
//! authoring trivial (touch a file, recompile) and lets `cargo`'s file
//! tracking invalidate the binary when a template changes — no
//! `build.rs` boilerplate, no second source of truth.
//!
//! ## Why a single `{name}` placeholder
//!
//! SPEC §10.1 names the project after the user's argument; downstream
//! validators (the slug rule in [`foglet_game::GameConfig`], Cargo's
//! crate-name rule) accept the same lowercase-alphanumeric-with-dashes
//! shape. One token covers the crate name, the slug, the README title,
//! and the Foglet door id without forcing the CLI to reason about
//! per-field transformations. Multi-token substitutions can be added
//! when a template grows a field that genuinely diverges from the slug.

/// One file the `fgk new` scaffolder writes into the destination
/// directory.
///
/// `dest_path` is **forward-slash separated** and always relative to
/// the project root; the scaffolder is responsible for translating to
/// the host's path separator and creating intermediate directories.
/// `contents` is the raw template text, including `{name}` tokens.
#[derive(Debug, Clone, Copy)]
pub struct Template {
    /// Destination path relative to the new project's root.
    pub dest_path: &'static str,
    /// File contents with unsubstituted `{name}` placeholders.
    pub contents: &'static str,
}

/// Every file `fgk new` writes for a fresh project.
///
/// Order is intentional but not load-bearing: `Cargo.toml` first so a
/// dry-run smoke test can fail fast on the most common scaffolding
/// breakage. The scaffolder MAY create directories on demand based on
/// `dest_path` rather than relying on this ordering.
pub const TEMPLATES: &[Template] = &[
    Template {
        dest_path: "Cargo.toml",
        contents: include_str!("../templates/Cargo.toml.tmpl"),
    },
    Template {
        dest_path: "src/main.rs",
        contents: include_str!("../templates/src/main.rs.tmpl"),
    },
    Template {
        dest_path: "assets/game.toml",
        contents: include_str!("../templates/assets/game.toml.tmpl"),
    },
    Template {
        dest_path: "assets/maps/start.txt",
        contents: include_str!("../templates/assets/maps/start.txt"),
    },
    // `.gitignore` lives on disk as `gitignore.tmpl` because `cargo
    // package` (and some editors) treat dotfile-prefixed templates as
    // local clutter to ignore. The destination keeps the leading dot.
    Template {
        dest_path: ".gitignore",
        contents: include_str!("../templates/gitignore.tmpl"),
    },
    Template {
        dest_path: "README.md",
        contents: include_str!("../templates/README.md.tmpl"),
    },
];

/// Replace every `{name}` placeholder in a template body.
///
/// Deliberately stupid: we do not parse the template text or guard
/// against `{name}` appearing inside string literals that meant to keep
/// the literal braces. The contract for template authors is "if you
/// write `{name}` in a template, you mean the substitution token". Any
/// future placeholder will get its own helper rather than evolving this
/// one into a mini templating engine.
pub fn substitute(contents: &str, name: &str) -> String {
    contents.replace("{name}", name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sample name used across the substitution tests. `test-game`
    /// satisfies the SPEC §9.1 slug rule (lowercase alphanumeric +
    /// `-`, no leading/trailing `-`) and Cargo's crate-name rule, so
    /// the substituted output should pass every parser we throw at it.
    const SAMPLE_NAME: &str = "test-game";

    #[test]
    fn templates_cover_spec_10_1_required_files() {
        // SPEC §10.1: `fgk new` MUST generate Cargo project, src/main.rs,
        // assets/game.toml, starter map, .gitignore, README.
        let paths: Vec<&str> = TEMPLATES.iter().map(|t| t.dest_path).collect();
        for required in [
            "Cargo.toml",
            "src/main.rs",
            "assets/game.toml",
            ".gitignore",
            "README.md",
        ] {
            assert!(
                paths.contains(&required),
                "missing required template: {required} (have {paths:?})"
            );
        }
        // Starter map: any file under assets/maps/ counts.
        assert!(
            paths.iter().any(|p| p.starts_with("assets/maps/")),
            "missing starter map under assets/maps/ (have {paths:?})"
        );
    }

    #[test]
    fn template_paths_are_unique_and_relative() {
        let mut seen = std::collections::HashSet::new();
        for t in TEMPLATES {
            assert!(
                !t.dest_path.starts_with('/'),
                "dest_path must be relative: {}",
                t.dest_path
            );
            assert!(
                !t.dest_path.contains(".."),
                "dest_path must not escape root: {}",
                t.dest_path
            );
            assert!(
                seen.insert(t.dest_path),
                "duplicate dest_path: {}",
                t.dest_path
            );
        }
    }

    #[test]
    fn substitute_replaces_every_name_token() {
        let out = substitute("hello {name}, welcome {name}!", "world");
        assert_eq!(out, "hello world, welcome world!");
    }

    #[test]
    fn substitute_leaves_text_without_token_untouched() {
        let out = substitute("no placeholders here", SAMPLE_NAME);
        assert_eq!(out, "no placeholders here");
    }

    fn rendered(dest_path: &str) -> String {
        let t = TEMPLATES
            .iter()
            .find(|t| t.dest_path == dest_path)
            .unwrap_or_else(|| panic!("template missing: {dest_path}"));
        let with_name = substitute(t.contents, SAMPLE_NAME);
        // `fgk new` resolves this token at scaffold time. Template-only
        // tests substitute a default crates.io line so parser checks can
        // stay focused on fixture validity rather than scaffolder wiring.
        with_name.replace("{foglet_game_dependency}", "foglet_game = \"0.1\"")
    }

    #[test]
    fn cargo_toml_template_parses_as_toml_after_substitution() {
        let body = rendered("Cargo.toml");
        let parsed: toml::Value =
            toml::from_str(&body).expect("rendered Cargo.toml should parse as TOML");
        let pkg = parsed
            .get("package")
            .and_then(|v| v.as_table())
            .expect("Cargo.toml has [package]");
        assert_eq!(
            pkg.get("name").and_then(|v| v.as_str()),
            Some(SAMPLE_NAME),
            "[package].name should be substituted with the project name"
        );
    }

    #[test]
    fn game_toml_template_parses_as_game_config_after_substitution() {
        let body = rendered("assets/game.toml");
        let cfg = foglet_game::GameConfig::from_toml_str(&body)
            .expect("rendered game.toml should pass GameConfig validation");
        // Title and slug both flow from `{name}`; if either drifts, the
        // scaffolder will produce a config the SPEC §9.1 validator
        // rejects.
        assert_eq!(cfg.game.slug, SAMPLE_NAME);
        assert_eq!(cfg.game.title, SAMPLE_NAME);
        assert!(cfg.game.min_width >= 80);
        assert!(cfg.game.min_height >= 24);
    }

    #[test]
    fn main_rs_template_parses_via_syn_after_substitution() {
        let body = rendered("src/main.rs");
        let file = syn::parse_file(&body)
            .unwrap_or_else(|e| panic!("rendered main.rs failed to parse: {e}"));
        // Sanity: at least one `fn main` so the binary will actually
        // build. We do not type-check here — that's `cargo test` in
        // 10b's smoke test. The intent is just "this is syntactically
        // a Rust file with an entry point".
        let has_main = file.items.iter().any(|item| match item {
            syn::Item::Fn(f) => f.sig.ident == "main",
            _ => false,
        });
        assert!(has_main, "main.rs template must define `fn main`");
    }

    #[test]
    fn starter_map_uses_only_legend_glyphs() {
        // The starter ships with a 3-glyph legend (`#` wall, `.`
        // floor, `@` player spawn). Asserting the map parses with that
        // legend keeps the template honest if a future edit introduces
        // a glyph without updating the legend.
        // Convention from `foglet_game::map`: the player spawn glyph
        // (`@`) maps to floor in the legend so the spawn cell is
        // walkable. Mirror that here so the starter map round-trips.
        let legend =
            foglet_game::TileLegend::from_pairs([("#", "wall"), (".", "floor"), ("@", "floor")])
                .expect("starter legend constructs");
        let map_text = TEMPLATES
            .iter()
            .find(|t| t.dest_path == "assets/maps/start.txt")
            .expect("starter map present")
            .contents;
        let map = foglet_game::parse_map(map_text, &legend)
            .expect("starter map should parse against its legend");
        assert!(map.in_bounds(0, 0));
    }

    #[test]
    fn gitignore_template_includes_target_and_save_dirs() {
        let body = rendered(".gitignore");
        for needle in ["/target/", "/.fgk/"] {
            assert!(
                body.contains(needle),
                ".gitignore should ignore {needle} (got: {body})"
            );
        }
    }

    #[test]
    fn readme_template_substitutes_name() {
        let body = rendered("README.md");
        assert!(
            body.contains(SAMPLE_NAME),
            "README should mention the project name after substitution"
        );
        assert!(
            !body.contains("{name}"),
            "README should not have unsubstituted `{{name}}` tokens"
        );
    }

    /// Regression guard for v1.1 Task 9d: the scaffolded README must
    /// surface the prompt primitives so authors know they're available
    /// without spelunking the kit's API docs.
    #[test]
    fn readme_template_mentions_prompt_primitives() {
        let body = rendered("README.md");
        for needle in ["ChoicePrompt", "ConfirmPrompt", "AnyKeyPrompt"] {
            assert!(
                body.contains(needle),
                "README should mention `{needle}` so authors discover the prompt API"
            );
        }
    }
}
