//! `turns` — daily turn ledger schema (SPEC_v2 §Task 6).
//!
//! Task 6a (this iteration) ships the `turn_ledger` migration only.
//! Subsequent sub-tasks layer behavior on top of the schema introduced
//! here:
//!
//! - 6b adds an injectable date provider so deterministic tests can
//!   simulate "tomorrow" without sleeping for 24 hours.
//! - 6c creates today's row from the configured `daily_allowance` the
//!   first time a player asks for their balance.
//! - 6d implements the atomic spend path.
//! - 6e rejects insufficient-turn spends without mutating the row.
//! - 6f handles new-day reset, including carryover capped by
//!   `[turns].carryover_max`.
//!
//! Splitting the migration into its own commit keeps the bisect signal
//! sharp: a regression that drops a column flunks the schema test in
//! this module rather than a higher-level spend assertion that's harder
//! to attribute. The migration is exported as a `pub const` so the
//! runtime startup path (Task 10) and game-author code can reference one
//! canonical definition without redeclaring the schema and drifting from
//! it — same pattern as [`crate::players::PLAYERS_MIGRATION`].
//!
//! # Why a per-(player, date) row instead of a single rolling balance
//!
//! SPEC_v2 §4.6 mandates four behaviors that all assume a notion of
//! "today's allowance":
//!
//! 1. Initialize a player with today's allowance.
//! 2. Spend turns atomically (today's balance shrinks).
//! 3. Reset on a new day (yesterday's row is no longer the active one).
//! 4. Carry over up to `carryover_max` to the new day.
//!
//! A single `players.balance` column would force the runtime to detect
//! "is today a new day?" *and* mutate balance in the same round-trip,
//! racing every other writer. Keeping one row per (`player_id`,
//! `local_date`) makes the active row a pure lookup (`WHERE player_id =
//! ? AND local_date = ?`) and reduces the carryover step (Task 6f) to
//! "read yesterday's balance, write today's row" — both single-row
//! operations the SPEC §9 transaction wrapper can compose without
//! touching unrelated rows.
//!
//! It also gives operators a queryable history: a sysop poking at
//! `sqlite3` can see *when* a player burned their turns rather than
//! just the current count. That's not a SPEC requirement, but it's a
//! free side-effect of the keying choice and worth not throwing away.

use std::fmt;

use thiserror::Error;

use crate::world_db::WorldMigration;

/// Schema for the daily turn ledger — SPEC_v2 §4.6 / §Task 6a.
///
/// One row per (`player_id`, `local_date`) pair. The "active" row for a
/// player on a given day is the one matching today's local date in the
/// configured timezone (Task 6b will introduce the date provider that
/// makes "today" testable). Old rows are retained on purpose so the
/// carryover step (6f) can read yesterday's balance directly and so
/// operators have a queryable history.
///
/// # Column shape
///
/// - `player_id` — `INTEGER NOT NULL REFERENCES players(id)`. Foreign
///   keyed to the player registry from Task 5 so the ledger can never
///   refer to a phantom identity. SQLite enforces foreign keys only
///   when `PRAGMA foreign_keys = ON`; the runtime layer (Task 10) is
///   responsible for enabling it at open time. Until then the constraint
///   is documentation, but the column shape is already correct so
///   enabling FK enforcement later is a one-line change rather than a
///   migration.
/// - `local_date` — `TEXT NOT NULL`. Stored as `YYYY-MM-DD` in the
///   server's local timezone (the only `[turns].reset` value v2 ships
///   is `local_midnight`). Text rather than `INTEGER` because the
///   sortable ISO format is human-readable in the `sqlite3` CLI and
///   round-trips cleanly through `chrono::NaiveDate` once Task 6b lands.
/// - `balance` — `INTEGER NOT NULL`. Remaining turns for that day.
///   Allowed to be zero (a player who burned every turn) but never
///   negative — Task 6e's insufficient-turn rejection lives in code
///   rather than as a `CHECK` constraint so the failure surfaces with
///   a typed error instead of `SQLITE_CONSTRAINT`. We could add the
///   `CHECK` belt-and-braces later; for now the simpler schema wins.
/// - `daily_allowance` — `INTEGER NOT NULL`. Snapshot of the
///   `[turns].daily_allowance` config value at the moment this row was
///   created. Stored (rather than recomputed from config) so an
///   operator who lowers the allowance mid-day doesn't retroactively
///   shrink yesterday's balances, and so the carryover step (6f) can
///   compute "unspent turns today" as `daily_allowance - balance` —
///   wait, that's only true when balance hasn't been touched by 6d
///   below. We store the original allowance to keep the row
///   self-describing for operators reading it cold.
/// - `created_at` / `updated_at` — UTC timestamps for audit. Defaulted
///   to `CURRENT_TIMESTAMP` so 6c can `INSERT` without threading a
///   clock; 6d updates `updated_at` on every spend.
///
/// # Primary key choice
///
/// `(player_id, local_date)` is the natural key — the SPEC's "keyed by
/// player and local date" wording in CHECKLIST_v2 §6a maps directly
/// onto a composite primary key. SQLite implements this as a
/// non-rowid covering index, so today-row lookups (`WHERE player_id =
/// ? AND local_date = ?`) are an index seek and the carryover lookup
/// (`WHERE player_id = ? AND local_date < ? ORDER BY local_date DESC
/// LIMIT 1`) is also seek-bound thanks to the left-prefix on
/// `player_id`.
///
/// Declaring it `WITHOUT ROWID` would shave a row of overhead per
/// entry; we don't, because (a) the table is small (one row per player
/// per day), (b) `WITHOUT ROWID` rules out future `RETURNING rowid`
/// patterns, and (c) the SPEC doesn't ask for it.
///
/// # Version
///
/// `version = 3`. Versions 1 and 2 are reserved for future kit-level
/// migrations and the players table respectively. Game-authored
/// migrations (Murder Motel's `motel_world_state` from Task 12a) start
/// from a higher band so they don't collide with kit migrations the
/// runtime applies on every open.
pub const TURN_LEDGER_MIGRATION: WorldMigration = WorldMigration {
    version: 3,
    name: "create_turn_ledger",
    sql: "\
CREATE TABLE IF NOT EXISTS turn_ledger (\n\
    player_id        INTEGER NOT NULL REFERENCES players(id),\n\
    local_date       TEXT NOT NULL,\n\
    balance          INTEGER NOT NULL,\n\
    daily_allowance  INTEGER NOT NULL,\n\
    created_at       TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    updated_at       TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    PRIMARY KEY (player_id, local_date)\n\
);\n\
",
};

/// A local calendar date in `YYYY-MM-DD` form — the value the
/// `turn_ledger.local_date` column stores and the unit of "today" the
/// rest of Task 6 keys off.
///
/// Wrapped in a newtype rather than passed around as a bare `String`
/// for two reasons:
///
/// 1. **Format invariant.** Once you hold a [`LocalDate`] you know the
///    string is exactly ten characters long, ASCII, and shaped like
///    `YYYY-MM-DD`. Task 6c's "today's row" lookup is a literal SQL
///    parameter bind, so any drift in shape (e.g. `2026-5-8` vs
///    `2026-05-08`) would silently miss rows. Validating once at the
///    boundary lets every consumer downstream compare with `==` and
///    sort lexically without re-checking.
/// 2. **Testability.** [`DateProvider`] returns `LocalDate`, never a
///    raw string, so a fixture date in a test is the same shape as a
///    production date — there is no "test-only string" branch to drift
///    apart from production.
///
/// The internal representation is a 10-byte `String` rather than a
/// `(year, month, day)` triple because the consumer (SQLite) ultimately
/// wants the ISO text anyway. Storing the canonical text avoids a
/// `format!()` allocation on every bind. Calendar math (Task 6f's
/// "yesterday" lookup) does not happen on `LocalDate` itself — it
/// happens via the SQL `WHERE local_date < ? ORDER BY local_date DESC
/// LIMIT 1` query SPEC §4.6 sketches, which only needs lexical
/// comparison and is exactly what the ISO format gives us for free.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalDate(String);

impl LocalDate {
    /// Construct a [`LocalDate`] from a string, validating ISO
    /// `YYYY-MM-DD` shape.
    ///
    /// Validation is intentionally **shape-only**: every position is an
    /// ASCII digit except the two `-` separators, and the string is
    /// exactly ten bytes long. This catches the realistic regression
    /// modes — a `format!()` macro that drops the zero pad, an upstream
    /// API returning `2026/05/08`, a stray newline — without pulling
    /// in a full calendar implementation that would also reject
    /// `2026-02-30` etc.
    ///
    /// We do *not* validate semantic legality (month <= 12, day-of-
    /// month bounds, leap years) at this layer because:
    ///
    /// - The only production [`DateProvider`] (Task 6c) will compute
    ///   the date from the system clock, which never produces an
    ///   illegal date.
    /// - Test code passing a date through this constructor is
    ///   deliberately picking it; rejecting `2026-02-30` would force
    ///   tests to know the calendar to write fixtures, which is busy
    ///   work without a regression to prevent.
    /// - SQLite stores the value as opaque text either way; a semantic
    ///   bug shows up in the test the date is meant to drive, not in a
    ///   constructor panic.
    ///
    /// If a later iteration *does* want strict calendar validation,
    /// it can be added without breaking callers — the constructor
    /// already returns `Result`.
    pub fn parse(input: impl Into<String>) -> Result<Self, TurnError> {
        let input = input.into();
        let bytes = input.as_bytes();
        let shape_ok = bytes.len() == 10
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes
                .iter()
                .enumerate()
                .all(|(i, b)| matches!(i, 4 | 7) || b.is_ascii_digit());
        if !shape_ok {
            return Err(TurnError::InvalidLocalDate { input });
        }
        Ok(Self(input))
    }

    /// Borrow the canonical `YYYY-MM-DD` representation. Use this when
    /// binding the date as a SQL parameter or comparing dates in tests.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the wrapper and return the owned `YYYY-MM-DD` string.
    /// Convenient for the SQLite layer when the call site already owns
    /// the [`LocalDate`] and would otherwise have to clone.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for LocalDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Library-internal turn-ledger errors — SPEC_v2 §Task 6.
///
/// `thiserror`-derived per the architecture tenet "thiserror inside
/// libraries". Variants are added as Tasks 6c–6f land. Task 6b only
/// needs the validation variant for [`LocalDate::parse`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TurnError {
    /// A date string was handed to [`LocalDate::parse`] that did not
    /// match `YYYY-MM-DD`. Surfaces the offending input verbatim so
    /// authors writing fixtures see *why* their date was rejected
    /// without having to re-derive the format from the doc comment.
    #[error("invalid local date {input:?}; expected YYYY-MM-DD")]
    InvalidLocalDate {
        /// The string that failed validation.
        input: String,
    },
}

/// Source of "what is today's local date" for the turn ledger.
///
/// SPEC_v2 §4.6 specifies daily allowance, atomic spend, midnight
/// reset, and capped carryover — every one of those behaviors is
/// gated on knowing today's date. Reading the system clock directly
/// inside the ledger code would make those behaviors untestable
/// (you'd have to wait until midnight to test reset). Routing the
/// answer through a trait lets:
///
/// - Production code use a system-clock-backed provider (introduced
///   in Task 6c when the first ledger-writing path actually consumes
///   it; deliberately not pre-built here to avoid landing dead code).
/// - Tests use [`FixedDateProvider`] to assert "first call on day 1
///   creates the row, second call on day 2 resets it" without
///   touching real wall-clock time.
///
/// # Why a trait rather than a function pointer or `dyn Fn`
///
/// A `Box<dyn Fn() -> LocalDate>` would also work and would save a
/// generic bound on every consumer. A trait is preferred because:
///
/// 1. The provider is a *role*, not a one-shot. A future iteration
///    might extend it with `now()` for event timestamps; a trait
///    accommodates that without rewriting every call site.
/// 2. Generic bounds (`fn spend<P: DateProvider>(p: &P, …)`) keep the
///    ledger paths monomorphizable and allocation-free, which matches
///    the rest of the kit's "library-internal hot paths avoid
///    `Box<dyn …>`" stance.
/// 3. Trait impls are self-documenting in `cargo doc` output — a
///    closure type on a public API is opaque to readers.
pub trait DateProvider {
    /// The local calendar date that should be treated as "today" for
    /// ledger reads/writes happening *right now*.
    ///
    /// Implementations must be cheap — the runtime calls this on every
    /// turn-spend and every screen render that displays remaining
    /// turns. Caching is the implementation's responsibility (the
    /// future production system-clock provider reads the clock each
    /// call; callers needing a single consistent date for a multi-step
    /// transaction should capture one [`LocalDate`] up front).
    fn today(&self) -> LocalDate;
}

/// Test fixture that always reports the same date.
///
/// Constructed once per scenario and handed to the ledger code under
/// test. Mutate via [`FixedDateProvider::set`] to simulate the clock
/// advancing — useful for the Task 6f "carryover on new day" test that
/// needs to write yesterday's row, then ask the ledger for today's
/// balance with a different date in the provider.
///
/// Lives in production code (not behind `#[cfg(test)]`) so example
/// programs and integration tests outside the `foglet_game` crate can
/// use it. SPEC §6 explicitly calls out that authoring code should be
/// able to drive the runtime deterministically; this is the date-side
/// piece of that contract.
#[derive(Debug, Clone)]
pub struct FixedDateProvider {
    date: LocalDate,
}

impl FixedDateProvider {
    /// Build a fixed-date provider returning `date` from every
    /// [`DateProvider::today`] call until [`FixedDateProvider::set`]
    /// changes it.
    pub fn new(date: LocalDate) -> Self {
        Self { date }
    }

    /// Replace the reported date. Used by Task 6f-flavored tests to
    /// simulate "the next day" without spinning up a second provider.
    /// Takes `&mut self` (rather than interior mutability) because the
    /// only callers are tests that own the provider; introducing a
    /// `Cell`/`Mutex` would buy nothing and obscure the simple shape.
    pub fn set(&mut self, date: LocalDate) {
        self.date = date;
    }
}

impl DateProvider for FixedDateProvider {
    fn today(&self) -> LocalDate {
        self.date.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::players::PLAYERS_MIGRATION;
    use crate::world_db::WorldDb;
    use tempfile::tempdir;

    /// SPEC_v2 §Task 6a acceptance: applying [`TURN_LEDGER_MIGRATION`]
    /// creates the documented `turn_ledger` table with the column shape
    /// later sub-tasks (6c–6f) depend on. Asserts both:
    ///
    /// 1. The table exists in `sqlite_master` (so a regression that
    ///    silently dropped the migration body would flunk).
    /// 2. The columns and order match the SPEC contract (so a later
    ///    edit that renames or reorders a column flunks here rather
    ///    than buried in a 6d spend test).
    ///
    /// We apply the players migration first because `turn_ledger`
    /// references it via `FOREIGN KEY`. With FK enforcement off (the
    /// SQLite default until Task 10 turns it on) the migration would
    /// succeed even without the parent table, but exercising the real
    /// dependency order here mirrors how the runtime startup path will
    /// drive migrations on a real door open.
    #[test]
    fn migration_creates_turn_ledger_table() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&TURN_LEDGER_MIGRATION)
            .expect("turn_ledger migration applies");

        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'turn_ledger'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master query runs");
        assert_eq!(
            count, 1,
            "turn_ledger table must exist after migration applies"
        );

        let mut stmt = world
            .connection()
            .prepare("SELECT name FROM pragma_table_info('turn_ledger') ORDER BY cid")
            .expect("pragma_table_info preparable");
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query_map runs")
            .collect::<Result<_, _>>()
            .expect("rows decode");
        assert_eq!(
            columns,
            vec![
                "player_id".to_string(),
                "local_date".to_string(),
                "balance".to_string(),
                "daily_allowance".to_string(),
                "created_at".to_string(),
                "updated_at".to_string(),
            ],
            "turn_ledger column shape must match the documented contract"
        );
    }

    /// The composite primary key `(player_id, local_date)` is what
    /// makes "today's row" a single index lookup and lets the Task 6f
    /// carryover step `SELECT … ORDER BY local_date DESC LIMIT 1`
    /// without a sequential scan. A regression that downgrades it to a
    /// simple `INTEGER PRIMARY KEY` (or drops the composite altogether)
    /// would silently degrade the ledger and let two rows for the same
    /// (player, day) pair coexist.
    #[test]
    fn turn_ledger_primary_key_is_player_id_and_local_date() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&TURN_LEDGER_MIGRATION)
            .expect("turn_ledger migration applies");

        // `pragma_table_info` reports the position of each column
        // within the primary key in its `pk` field (1-based, 0 for
        // non-PK columns). Querying it directly is more robust than
        // parsing `sqlite_master.sql`, which formats the key in
        // implementation-defined whitespace.
        let mut stmt = world
            .connection()
            .prepare(
                "SELECT name, pk FROM pragma_table_info('turn_ledger') \
                 WHERE pk > 0 ORDER BY pk",
            )
            .expect("pragma_table_info preparable");
        let pk_columns: Vec<(String, i64)> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .expect("query_map runs")
            .collect::<Result<_, _>>()
            .expect("rows decode");
        assert_eq!(
            pk_columns,
            vec![("player_id".to_string(), 1), ("local_date".to_string(), 2),],
            "primary key must be (player_id, local_date) in that order"
        );
    }

    /// Inserting two rows that share `(player_id, local_date)` must
    /// raise a uniqueness error — the contract Task 6c–6f relies on
    /// when it reads "today's row" without first locking the table.
    /// Without this guarantee a race between two writers could leave
    /// the ledger with two contradictory balance rows for the same
    /// (player, day) pair and SPEC §4.6's "spend turns atomically"
    /// promise would be unenforceable.
    #[test]
    fn turn_ledger_rejects_duplicate_player_day_rows() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&TURN_LEDGER_MIGRATION)
            .expect("turn_ledger migration applies");

        // Insert a parent player so the FK column has something to
        // reference. We don't enable `PRAGMA foreign_keys` (Task 10's
        // job) so the parent isn't strictly required, but writing a
        // realistic row keeps the test true to how the runtime will
        // drive the table.
        world
            .connection()
            .execute("INSERT INTO players (id, handle) VALUES (1, 'alice')", [])
            .expect("seed player row");

        world
            .connection()
            .execute(
                "INSERT INTO turn_ledger (player_id, local_date, balance, daily_allowance) \
                 VALUES (1, '2026-05-08', 30, 30)",
                [],
            )
            .expect("first ledger row inserts");

        let err = world
            .connection()
            .execute(
                "INSERT INTO turn_ledger (player_id, local_date, balance, daily_allowance) \
                 VALUES (1, '2026-05-08', 29, 30)",
                [],
            )
            .expect_err("duplicate (player_id, local_date) must be rejected");

        // Don't pin the exact `rusqlite::Error` variant — SQLite's
        // wording around constraint violations evolves between
        // versions. Asserting the message names the table is enough
        // for a regression to point at the right spot.
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("turn_ledger") || msg.to_lowercase().contains("unique"),
            "duplicate insert error should mention turn_ledger or uniqueness, got: {msg}"
        );
    }

    /// Canonical `YYYY-MM-DD` strings round-trip through
    /// [`LocalDate::parse`] without modification. The parsed value's
    /// `as_str()` matches the input verbatim — this is the contract the
    /// SQL parameter binder relies on (Task 6c–6f: bind today's date,
    /// match yesterday's row by lexical comparison).
    #[test]
    fn local_date_parse_accepts_canonical_iso_form() {
        let date = LocalDate::parse("2026-05-08").expect("canonical date parses");
        assert_eq!(date.as_str(), "2026-05-08");
        // Display matches as_str so format!() in messages is safe.
        assert_eq!(format!("{date}"), "2026-05-08");
        // into_string yields the same canonical text without copying.
        assert_eq!(date.into_string(), "2026-05-08");
    }

    /// [`LocalDate::parse`] rejects every realistic shape regression we
    /// expect to see: missing zero pad, alternate separators, extra
    /// whitespace, wrong length, non-digit content. Each case has a
    /// real-world failure mode behind it (manual `format!()`,
    /// upstream API drift, copy-paste from a log line) so a regression
    /// in the validator surfaces as a named scenario rather than a
    /// vague "string did not parse".
    #[test]
    fn local_date_parse_rejects_malformed_inputs() {
        let bad = [
            "",            // empty
            "2026-5-08",   // missing zero pad on month
            "2026-05-8",   // missing zero pad on day
            "2026/05/08",  // wrong separator
            "26-05-08",    // 2-digit year
            "2026-05-08 ", // trailing whitespace
            " 2026-05-08", // leading whitespace
            "2026-05-08T", // length 11
            "abcd-ef-gh",  // non-digit content
        ];
        for input in bad {
            let err =
                LocalDate::parse(input).expect_err(&format!("expected {input:?} to be rejected"));
            let TurnError::InvalidLocalDate { input: got } = &err;
            assert_eq!(
                got, input,
                "InvalidLocalDate should preserve the offending input verbatim"
            );
            // The Display impl mentions the input so authors writing
            // fixtures see why the date was rejected without re-reading
            // the doc comment.
            let msg = err.to_string();
            assert!(
                msg.contains("YYYY-MM-DD"),
                "error message should mention expected format, got: {msg}"
            );
        }
    }

    /// [`FixedDateProvider`] returns the configured date from every
    /// [`DateProvider::today`] call. This is the bedrock contract Task
    /// 6c–6f tests rely on: hand the ledger a fixed provider, drive
    /// it, observe deterministic behavior.
    #[test]
    fn fixed_date_provider_returns_configured_date() {
        let provider =
            FixedDateProvider::new(LocalDate::parse("2026-05-08").expect("fixture date parses"));
        assert_eq!(provider.today().as_str(), "2026-05-08");
        // Repeated calls return the same value — no hidden mutation.
        assert_eq!(provider.today().as_str(), "2026-05-08");
    }

    /// [`FixedDateProvider::set`] simulates the clock advancing — the
    /// shape Task 6f's "carryover on a new day" test will use to
    /// write yesterday's row, advance the clock, then ask for today's
    /// balance and assert the carryover ran. Asserting the behavior
    /// here pins the contract before the consumer lands.
    #[test]
    fn fixed_date_provider_set_advances_reported_date() {
        let mut provider =
            FixedDateProvider::new(LocalDate::parse("2026-05-08").expect("first date parses"));
        assert_eq!(provider.today().as_str(), "2026-05-08");

        provider.set(LocalDate::parse("2026-05-09").expect("second date parses"));
        assert_eq!(provider.today().as_str(), "2026-05-09");
    }

    /// [`DateProvider`] is dispatchable through a generic bound — the
    /// shape Task 6c+ ledger functions will use (`fn ensure_today<P:
    /// DateProvider>(p: &P, …)`). A regression that accidentally tied
    /// the trait to a `Self: Sized` bound or otherwise broke generic
    /// usage would be caught here rather than in a downstream
    /// consumer.
    #[test]
    fn date_provider_is_usable_through_generic_bound() {
        fn read<P: DateProvider>(p: &P) -> String {
            p.today().into_string()
        }
        let provider =
            FixedDateProvider::new(LocalDate::parse("2026-05-08").expect("fixture date parses"));
        assert_eq!(read(&provider), "2026-05-08");
    }
}
