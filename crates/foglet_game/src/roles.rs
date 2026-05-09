//! `roles` — typed role normalization for Foglet doors (SPEC_v2 §4.5).
//!
//! Modern Rust games keep BBS-era "security level" semantics for
//! flavor, sysop affordances, and dropfile-compatible permission
//! gates. Foglet itself reports a free-form `role` string on the
//! [`FogletContext`]; this module normalizes that string into a typed
//! [`FogletRole`] and maps it to the canonical integer
//! [`security_level`](FogletRole::security_level).
//!
//! ## Why this is its own module
//!
//! SPEC_v2 §3 lists `roles` as a top-level kit module, peer to
//! `world_db`, `players`, `turns`, `events`, and `leaderboards`. The
//! parsing rule and the integer mapping live in code (rather than in
//! SQL defaults or in the `players` schema) so that:
//!
//! - the rule has exactly one home — change "mod = 90" here and every
//!   downstream use picks up the new mapping without a schema migration;
//! - games that never open a world DB can still call
//!   [`FogletContext::security_level`] for sysop/debug affordances;
//! - Task 5f (persisting normalized role/security at upsert time) reads
//!   from this module instead of duplicating the mapping in `players.rs`.
//!
//! ## Authority boundary
//!
//! SPEC §4.5 is explicit: role/security values are **advisory** game
//! metadata. They MUST NOT be used as launch authorization. Foglet
//! remains the single source of truth for whether a user may open a
//! door; games consult [`FogletRole`] only for in-game flavor, sysop
//! menus, and dropfile-compatible permission flags.

use serde::{Deserialize, Serialize};

use crate::foglet::FogletContext;

/// Normalized Foglet role.
///
/// Mirrors SPEC_v2 §4.5's enum exactly. `Other(String)` preserves the
/// original spelling of an unrecognised role so operator-facing UI
/// can show "you appear to be in role 'beta-tester'" rather than
/// silently collapsing it to `User`. The mapping in
/// [`Self::security_level`] still treats unknowns as user-level (50)
/// per the spec.
///
/// # Wire format
///
/// `serde` is wired with `rename_all = "snake_case"` so the enum
/// round-trips through the same lowercase strings Foglet emits
/// (`"sysop"`, `"mod"`, `"user"`). `Other(String)` carries its payload
/// verbatim — when serialised it becomes `{"other":"beta-tester"}`,
/// matching the conventional serde untagged-payload shape. This is
/// deliberate: deserialising back from the same JSON has to be
/// lossless, and a string-only encoding would conflict with the
/// known variants on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FogletRole {
    /// Standard user — the default role and the security-level floor.
    User,
    /// Moderator — elevated above users for in-game permission gates
    /// like clue review or chat moderation. SPEC §4.5 maps to 90.
    Mod,
    /// Sysop — top-of-stack role for debug menus, world-state
    /// inspectors, and any "operator-only" affordance. SPEC §4.5
    /// maps to 100.
    Sysop,
    /// Any role string Foglet supplies that we don't recognise.
    /// Carries the original (case-preserved) string so games can
    /// surface "unknown role: foo" diagnostics in sysop menus.
    Other(String),
}

/// Security-level integer for a [`FogletRole::User`] (and any unknown
/// role). Exposed as a `pub const` so downstream code can compare
/// against it without duplicating the literal — SPEC §4.5 calls this
/// the "default" mapping and a renumber would land here once.
pub const USER_SECURITY_LEVEL: i64 = 50;

/// Security-level integer for a [`FogletRole::Mod`].
pub const MOD_SECURITY_LEVEL: i64 = 90;

/// Security-level integer for a [`FogletRole::Sysop`].
pub const SYSOP_SECURITY_LEVEL: i64 = 100;

impl FogletRole {
    /// Parse a role string per SPEC §4.5: case-insensitive, with
    /// surrounding whitespace trimmed (Foglet's contract doesn't
    /// promise tidy strings, and a stray newline shouldn't downgrade
    /// a sysop to "Other").
    ///
    /// Unknown strings land in [`FogletRole::Other`]; the spec says
    /// unknown roles MUST map to the user-level integer, but the
    /// original string is still useful to preserve for diagnostics.
    /// Empty / whitespace-only strings collapse to [`FogletRole::User`]
    /// — equivalent to the "absent role" path on
    /// [`FogletContext::role`], which keeps the two entry points
    /// returning the same value for the same logical input.
    pub fn parse(raw: &str) -> Self {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return FogletRole::User;
        }
        // ASCII-lower is sufficient: Foglet's role strings are ASCII
        // tokens (sysop / mod / user). Going through `to_lowercase`
        // would needlessly allocate on the happy path; `eq_ignore_ascii_case`
        // matches the documented variants without lower-casing the input,
        // and we keep the original spelling for the `Other` fallback.
        if trimmed.eq_ignore_ascii_case("sysop") {
            FogletRole::Sysop
        } else if trimmed.eq_ignore_ascii_case("mod") {
            FogletRole::Mod
        } else if trimmed.eq_ignore_ascii_case("user") {
            FogletRole::User
        } else {
            FogletRole::Other(trimmed.to_owned())
        }
    }

    /// Mapped security-level integer per SPEC §4.5.
    ///
    /// `Other` collapses to [`USER_SECURITY_LEVEL`] — the spec says
    /// "user or absent/unknown -> 50" without exception. A future
    /// adapter that decides to escalate specific known-other roles
    /// would do so by promoting them into a typed variant first
    /// (preserving the "one mapping in one place" invariant) rather
    /// than special-casing the `Other` arm here.
    pub fn security_level(&self) -> i64 {
        match self {
            FogletRole::Sysop => SYSOP_SECURITY_LEVEL,
            FogletRole::Mod => MOD_SECURITY_LEVEL,
            FogletRole::User | FogletRole::Other(_) => USER_SECURITY_LEVEL,
        }
    }

    /// Canonical lowercase token for the role.
    ///
    /// Used by Task 5f when persisting the normalized role to the
    /// `players.role` column so the on-disk value is the same shape a
    /// SPEC §4.5 reader would expect (`'sysop'` / `'mod'` / `'user'`).
    /// `Other(s)` returns the trimmed original string verbatim — a
    /// dropfile-compat layer that wants to preserve unknown roles in
    /// the registry can do so without losing fidelity.
    pub fn as_token(&self) -> &str {
        match self {
            FogletRole::Sysop => "sysop",
            FogletRole::Mod => "mod",
            FogletRole::User => "user",
            FogletRole::Other(s) => s.as_str(),
        }
    }
}

impl FogletContext {
    /// Normalized [`FogletRole`] for this context.
    ///
    /// Equivalent to `FogletRole::parse(self.role.as_deref().unwrap_or(""))`.
    /// An absent `role` field maps to [`FogletRole::User`] (the SPEC
    /// §4.5 "absent" path), matching the SQL default in
    /// `PLAYERS_MIGRATION`.
    pub fn foglet_role(&self) -> FogletRole {
        match self.role.as_deref() {
            Some(raw) => FogletRole::parse(raw),
            None => FogletRole::User,
        }
    }

    /// Mapped security-level integer per SPEC §4.5.
    ///
    /// Convenience wrapper around `self.foglet_role().security_level()`
    /// — most callers want the integer for a dropfile-compat gate
    /// (`if ctx.security_level() >= 90 { … }`) and shouldn't have to
    /// import [`FogletRole`] just to bridge to it.
    ///
    /// SPEC §4.5 leaves a future hook for an explicit
    /// `security_level` field on the wire; when that lands the typed
    /// value will override the role-derived mapping here. For now the
    /// derivation is the single source of truth.
    pub fn security_level(&self) -> i64 {
        self.foglet_role().security_level()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foglet::ContextSource;

    /// Helper: build a context whose only meaningful field is `role`.
    /// The other fields are stage dressing — the methods under test
    /// only read `role`.
    fn ctx_with_role(role: Option<&str>) -> FogletContext {
        FogletContext {
            door_id: "test-door".to_string(),
            user_id: None,
            username: None,
            role: role.map(str::to_string),
            session_id: None,
            terminal_width: 80,
            terminal_height: 24,
            source: ContextSource::ContextFile,
        }
    }

    /// Canonical lowercase tokens parse to the documented variants.
    #[test]
    fn parse_canonical_tokens() {
        assert_eq!(FogletRole::parse("sysop"), FogletRole::Sysop);
        assert_eq!(FogletRole::parse("mod"), FogletRole::Mod);
        assert_eq!(FogletRole::parse("user"), FogletRole::User);
    }

    /// SPEC §4.5: parsing is case-insensitive. A Foglet adapter that
    /// emits `"Sysop"` or `"SYSOP"` must still resolve to
    /// [`FogletRole::Sysop`].
    #[test]
    fn parse_is_case_insensitive() {
        assert_eq!(FogletRole::parse("SYSOP"), FogletRole::Sysop);
        assert_eq!(FogletRole::parse("Sysop"), FogletRole::Sysop);
        assert_eq!(FogletRole::parse("sYsOp"), FogletRole::Sysop);
        assert_eq!(FogletRole::parse("MOD"), FogletRole::Mod);
        assert_eq!(FogletRole::parse("Mod"), FogletRole::Mod);
        assert_eq!(FogletRole::parse("USER"), FogletRole::User);
        assert_eq!(FogletRole::parse("User"), FogletRole::User);
    }

    /// Surrounding whitespace is trimmed before matching — Foglet's
    /// contract doesn't promise tidy strings and a `\nsysop\n` from a
    /// shell heredoc shouldn't be mistaken for an unknown role.
    #[test]
    fn parse_trims_whitespace() {
        assert_eq!(FogletRole::parse("  sysop  "), FogletRole::Sysop);
        assert_eq!(FogletRole::parse("\nmod\t"), FogletRole::Mod);
    }

    /// Unknown roles land in [`FogletRole::Other`] preserving the
    /// trimmed original string. A future sysop UI can render
    /// "you appear to be in role X" without losing the spelling.
    #[test]
    fn parse_unknown_role_preserves_original_string() {
        assert_eq!(
            FogletRole::parse("beta-tester"),
            FogletRole::Other("beta-tester".to_owned())
        );
        assert_eq!(
            FogletRole::parse("  Auditor  "),
            FogletRole::Other("Auditor".to_owned()),
            "case is preserved on unknown roles even though matching is case-insensitive"
        );
    }

    /// Empty / whitespace-only strings collapse to
    /// [`FogletRole::User`], matching the "absent role" path on
    /// [`FogletContext::foglet_role`].
    #[test]
    fn parse_empty_or_whitespace_falls_back_to_user() {
        assert_eq!(FogletRole::parse(""), FogletRole::User);
        assert_eq!(FogletRole::parse("   "), FogletRole::User);
        assert_eq!(FogletRole::parse("\t\n"), FogletRole::User);
    }

    /// SPEC §4.5 mapping: sysop=100, mod=90, user=50.
    #[test]
    fn security_level_mapping_matches_spec() {
        assert_eq!(FogletRole::Sysop.security_level(), 100);
        assert_eq!(FogletRole::Mod.security_level(), 90);
        assert_eq!(FogletRole::User.security_level(), 50);
    }

    /// Unknown roles map to user-level (50). The spec is explicit
    /// about this: "user or absent/unknown -> 50".
    #[test]
    fn unknown_role_maps_to_user_security_level() {
        assert_eq!(
            FogletRole::Other("auditor".to_owned()).security_level(),
            USER_SECURITY_LEVEL
        );
    }

    /// `as_token` yields the canonical lowercase spelling for the
    /// known variants — exactly what Task 5f will write into
    /// `players.role`.
    #[test]
    fn as_token_returns_canonical_spelling() {
        assert_eq!(FogletRole::Sysop.as_token(), "sysop");
        assert_eq!(FogletRole::Mod.as_token(), "mod");
        assert_eq!(FogletRole::User.as_token(), "user");
        assert_eq!(
            FogletRole::Other("beta-tester".to_owned()).as_token(),
            "beta-tester"
        );
    }

    /// `FogletContext::foglet_role` resolves a missing `role` field
    /// to `User` — equivalent to the empty-string parse path.
    #[test]
    fn context_with_no_role_returns_user() {
        assert_eq!(ctx_with_role(None).foglet_role(), FogletRole::User);
        assert_eq!(ctx_with_role(None).security_level(), USER_SECURITY_LEVEL);
    }

    /// `FogletContext::security_level` walks through the role parser
    /// and returns the mapped integer.
    #[test]
    fn context_security_level_uses_role_mapping() {
        assert_eq!(ctx_with_role(Some("sysop")).security_level(), 100);
        assert_eq!(ctx_with_role(Some("MOD")).security_level(), 90);
        assert_eq!(ctx_with_role(Some("user")).security_level(), 50);
        assert_eq!(
            ctx_with_role(Some("auditor")).security_level(),
            USER_SECURITY_LEVEL,
            "unknown role on context still maps to user-level"
        );
        assert_eq!(
            ctx_with_role(Some("")).security_level(),
            USER_SECURITY_LEVEL,
            "empty role on context maps to user-level"
        );
    }

    /// Round-trip the known variants through serde — Task 5f and any
    /// future structured event metadata that embeds a `FogletRole`
    /// rely on the snake_case wire form documented above.
    #[test]
    fn known_variants_round_trip_through_serde() {
        for role in [FogletRole::Sysop, FogletRole::Mod, FogletRole::User] {
            let json = serde_json::to_string(&role).expect("serialize");
            let back: FogletRole = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, role, "round-trip for {role:?} via {json}");
        }
    }
}
