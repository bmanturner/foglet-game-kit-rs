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

use crate::world_db::{WorldDb, WorldMigration};

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
/// libraries". Variants are added as Tasks 6c–6f land. Task 6b
/// introduced the validation variant for [`LocalDate::parse`]; Task 6c
/// adds [`TurnError::Sqlite`] for failures coming out of the SQLite
/// driver while reading or writing today's row.
#[derive(Debug, Error)]
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
    /// The SQLite round-trip backing a turn-ledger read or write failed.
    /// Wraps `rusqlite::Error` rather than re-wording it so `tracing`
    /// and the operator-facing `anyhow` boundary in Task 10 keep the
    /// underlying SQLite reason intact.
    #[error("turn ledger SQLite operation failed: {source}")]
    Sqlite {
        /// Underlying `rusqlite` error from the offending statement.
        #[source]
        source: rusqlite::Error,
    },
    /// A spend was rejected because today's balance is below the
    /// requested amount — SPEC_v2 §Task 6e. The persisted balance is
    /// **not** mutated when this variant is returned; callers can
    /// surface `balance` to the player as "you only have N turns
    /// left" without re-querying.
    ///
    /// `requested` is the unsigned amount the caller asked to spend,
    /// widened to `i64` so the error formats consistently with
    /// `balance` (which is the SQLite `INTEGER` storage type).
    #[error(
        "insufficient turns for player {player_id}: requested {requested}, \
         balance is {balance}"
    )]
    InsufficientTurns {
        /// Player whose row was checked.
        player_id: i64,
        /// Current balance at the moment the spend was rejected.
        balance: i64,
        /// Amount the caller asked to spend.
        requested: i64,
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

/// Snapshot of a single `turn_ledger` row — what every Task 6
/// operation hands back to the caller.
///
/// The struct is intentionally a flat data carrier rather than a handle
/// onto the database. Once the caller has a [`TurnLedgerRow`] the
/// connection is free for the next statement; the runtime layer
/// (Task 10) needs that property because screen renders display
/// remaining turns without keeping a write lock open.
///
/// Field types mirror the SQLite columns: `i64` for the integer
/// counters (matches `INTEGER` storage class without a narrowing cast)
/// and a [`LocalDate`] for the calendar day so consumers downstream
/// keep the validated shape rather than a bare string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnLedgerRow {
    /// Foreign key into `players.id` — the player this row belongs to.
    pub player_id: i64,
    /// The local calendar date this row tracks (`YYYY-MM-DD`).
    pub local_date: LocalDate,
    /// Remaining turns for `(player_id, local_date)`. Equal to
    /// `daily_allowance` immediately after Task 6c creates the row;
    /// Task 6d's spend path will decrement it.
    pub balance: i64,
    /// Snapshot of `[turns].daily_allowance` at the moment this row
    /// was created. Preserved across the day so an operator who
    /// edits the config mid-day doesn't retroactively shrink the
    /// balance the player already saw.
    pub daily_allowance: i64,
}

impl WorldDb {
    /// Return today's [`TurnLedgerRow`] for `player_id`, creating the
    /// row from `daily_allowance` on its first read of the day —
    /// SPEC_v2 §Task 6c "initial daily allowance creation".
    ///
    /// Two callers in v2:
    ///
    /// 1. The screen-render path that displays "remaining turns"
    ///    needs to know how many turns the player has *right now*.
    ///    On the first call of a given day the row doesn't exist
    ///    yet, so this function lazily writes it from the configured
    ///    allowance.
    /// 2. The spend path (Task 6d) reads today's balance before
    ///    decrementing. Its preflight is the same "make sure today's
    ///    row exists" question, which is exactly what this method
    ///    answers — Task 6d will compose with this rather than open-
    ///    coding the ensure-then-spend dance.
    ///
    /// # Why `INSERT OR IGNORE` then `SELECT`
    ///
    /// The function must be safe under contention: two screens
    /// rendering for the same player at boot must not both insert
    /// the row. `INSERT OR IGNORE` makes "row already exists" a
    /// no-op at the SQLite layer rather than a typed error, and the
    /// follow-up `SELECT` returns whichever row is now there —
    /// either the one we wrote or the one a sibling writer beat us
    /// to. That keeps a stale or in-progress balance from being
    /// clobbered with a fresh `daily_allowance`, which is the bug
    /// `INSERT … ON CONFLICT DO UPDATE` would silently introduce.
    ///
    /// We deliberately *don't* wrap the two statements in a
    /// transaction. The `INSERT` is atomic on its own, the `SELECT`
    /// is read-only, and the busy timeout configured at open time
    /// (SPEC §3) handles the only contention story. Adding a
    /// transaction here would buy nothing while making the function
    /// require `&mut self`, which would fight the runtime layer's
    /// borrow shape (Task 10 holds the world DB by shared reference
    /// from the screen render path).
    ///
    /// # Why the date provider rather than a `&LocalDate`
    ///
    /// Taking `&P: DateProvider` matches the shape Tasks 6d–6f will
    /// reach for: every ledger operation asks "what's today?" at the
    /// moment of the call. A `&LocalDate` parameter would force the
    /// caller to query the provider, which is fine for one call site
    /// but turns into noise once spend, reset, and carryover all
    /// thread the same provider through.
    ///
    /// Generic dispatch is monomorphized — the production
    /// system-clock provider and the test [`FixedDateProvider`] both
    /// inline the call without a `Box<dyn …>` indirection.
    ///
    /// # Errors
    ///
    /// Returns [`TurnError::Sqlite`] if either the insert-or-ignore
    /// or the follow-up select fails (table missing, FK violation
    /// when foreign keys are enabled, IO error). The variant wraps
    /// the original `rusqlite::Error` so the operator-facing message
    /// in Task 10 keeps SQLite's wording.
    ///
    /// # Idempotency
    ///
    /// Calling twice on the same `(player_id, local_date)` returns
    /// the same row both times — including any spend that landed
    /// between the two calls (Task 6d). The function therefore
    /// doubles as a "read today's balance" query for callers that
    /// don't care whether the row was just created or already
    /// existed.
    pub fn ensure_today_turns<P: DateProvider>(
        &self,
        player_id: i64,
        daily_allowance: u32,
        date_provider: &P,
    ) -> Result<TurnLedgerRow, TurnError> {
        let today = date_provider.today();

        // Cast `u32 → i64` once: SQLite stores INTEGER as 64-bit
        // signed, and the cast cannot overflow because `u32::MAX <
        // i64::MAX`. Doing it here keeps the bind sites below from
        // sprouting `as i64` noise.
        let allowance = i64::from(daily_allowance);

        // INSERT OR IGNORE: if the (player_id, local_date) pair is
        // already present (because we ran earlier today, or a
        // sibling render path raced in front of us), this is a
        // no-op. The follow-up SELECT then returns whichever row is
        // there — preserving any spend that landed in between.
        self.connection()
            .execute(
                "INSERT OR IGNORE INTO turn_ledger \
                 (player_id, local_date, balance, daily_allowance) \
                 VALUES (?1, ?2, ?3, ?3)",
                rusqlite::params![player_id, today.as_str(), allowance],
            )
            .map_err(|source| TurnError::Sqlite { source })?;

        // SELECT today's row. We read both `balance` and
        // `daily_allowance` rather than assuming `balance ==
        // allowance`: on the no-op branch above the balance may
        // already be lower than `daily_allowance`, and the stored
        // allowance is whatever was configured when *this* row was
        // created (not whatever the caller passed in just now).
        let (balance, stored_allowance): (i64, i64) = self
            .connection()
            .query_row(
                "SELECT balance, daily_allowance FROM turn_ledger \
                 WHERE player_id = ?1 AND local_date = ?2",
                rusqlite::params![player_id, today.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|source| TurnError::Sqlite { source })?;

        Ok(TurnLedgerRow {
            player_id,
            local_date: today,
            balance,
            daily_allowance: stored_allowance,
        })
    }

    /// Atomically decrement today's balance for `player_id` by
    /// `amount` and return the resulting [`TurnLedgerRow`] —
    /// SPEC_v2 §Task 6d "atomic turn spend".
    ///
    /// Composes with [`WorldDb::ensure_today_turns`]: if today's row
    /// does not yet exist (first action of the day), it is materialised
    /// at the configured `daily_allowance` *before* the spend lands.
    /// Game code therefore only needs one call to "burn a turn" — the
    /// lazy initialisation that Task 6c set up is plumbed in here so
    /// callers don't have to remember the two-step dance.
    ///
    /// # Atomicity
    ///
    /// The decrement is a single SQL `UPDATE` of the form
    /// `SET balance = balance - ?`. SQLite serialises writes per
    /// connection, and the `WHERE player_id = ? AND local_date = ?`
    /// clause matches exactly the one row keyed by the composite
    /// primary key — no cursor walks, no read-then-write race. Two
    /// concurrent spenders queued on the busy timeout therefore see
    /// the second decrement applied to the *result* of the first,
    /// rather than both reading the same balance and clobbering each
    /// other. SPEC §4.6 calls that property out as a hard requirement
    /// for the ledger; the single-statement form delivers it without
    /// an explicit transaction.
    ///
    /// # Insufficient-turn rejection (SPEC_v2 §Task 6e)
    ///
    /// The `UPDATE` carries a `WHERE balance >= ?` guard so a spend
    /// that would push the balance below zero matches zero rows
    /// instead of running. We detect the zero-row case via
    /// `Connection::execute`'s row count, re-read the canonical
    /// balance for the error payload, and return
    /// [`TurnError::InsufficientTurns`]. The persisted row is
    /// guaranteed untouched on this path: the guard prevents the
    /// `UPDATE` from landing, and the read-back `SELECT` only ever
    /// reads. Callers can therefore display "you only have N turns
    /// left" straight from the error variant without a follow-up
    /// query.
    ///
    /// # What this method does *not* do (yet)
    ///
    /// - **New-day reset / carryover.** Task 6f. The spend path
    ///   always operates on *today's* row as reported by the
    ///   `date_provider`; if the date has rolled over since the last
    ///   spend, `ensure_today_turns` materialises a fresh row at full
    ///   allowance and the decrement applies to it. Carryover from
    ///   yesterday's leftover balance is layered on in 6f.
    ///
    /// # Why a separate `SELECT` after the `UPDATE`
    ///
    /// We could use SQLite 3.35+'s `UPDATE … RETURNING` clause to
    /// fold the decrement and the readback into one round-trip.
    /// We don't, for two reasons:
    ///
    /// 1. The supported SQLite version floor is set by `rusqlite`'s
    ///    bundled feature being **off** in our crate budget (ADR
    ///    documented in `DECISIONS.md`). Relying on a newer SQL
    ///    feature would silently break on hosts that ship an older
    ///    system SQLite. A plain `SELECT` is portable to every
    ///    SQLite version we support.
    /// 2. The two-statement form keeps `ensure_today_turns`'s
    ///    select-the-current-row code path the single source of
    ///    truth for "what does a `TurnLedgerRow` look like coming
    ///    out of the DB?". When 6e adds insufficient-turn rejection,
    ///    or 6f adds carryover, the readback shape stays stable.
    ///
    /// # Errors
    ///
    /// - [`TurnError::Sqlite`] if any of the round-trips (ensure-row,
    ///   update, readback) fails. The variant wraps the underlying
    ///   `rusqlite::Error` so the operator-facing layer (Task 10) can
    ///   surface SQLite's wording verbatim.
    /// - [`TurnError::InsufficientTurns`] if today's balance is below
    ///   `amount`. The persisted row is unchanged when this is
    ///   returned; the variant carries the current `balance` and the
    ///   `requested` amount so callers can render a player-facing
    ///   message directly from the error.
    pub fn spend_turns<P: DateProvider>(
        &self,
        player_id: i64,
        amount: u32,
        daily_allowance: u32,
        date_provider: &P,
    ) -> Result<TurnLedgerRow, TurnError> {
        // Materialise today's row first (no-op if it already exists).
        // Capturing the returned row gives us the canonical date the
        // provider reported, so the subsequent UPDATE binds the same
        // string the row was keyed under — no risk of the provider
        // returning a different value between calls in a misbehaving
        // implementation.
        let row = self.ensure_today_turns(player_id, daily_allowance, date_provider)?;

        // Cast amount once — SQLite stores INTEGER as 64-bit signed,
        // u32 → i64 is infallible.
        let delta = i64::from(amount);

        // Single atomic UPDATE: `balance = balance - ?`. The WHERE
        // clause matches exactly one row via the composite primary
        // key, so this is the per-row atomic decrement SPEC §4.6
        // requires. `updated_at = CURRENT_TIMESTAMP` keeps the audit
        // column meaningful — operators reading the table cold see
        // when the last spend landed.
        //
        // The `balance >= ?1` guard is the SPEC_v2 §Task 6e
        // insufficient-turn check. Folding it into the same UPDATE
        // (rather than checking `row.balance` from
        // `ensure_today_turns` and branching in Rust) lets SQLite
        // serialise the read-and-write atomically — even with a
        // sibling connection racing through its own busy-timeout
        // queue, only one of the two spends can satisfy the guard.
        // A zero-amount spend still matches because `balance >= 0`
        // is trivially true; the existing 6d "zero amount is a
        // no-op" contract is preserved.
        let updated = self
            .connection()
            .execute(
                "UPDATE turn_ledger \
                 SET balance = balance - ?1, updated_at = CURRENT_TIMESTAMP \
                 WHERE player_id = ?2 AND local_date = ?3 AND balance >= ?1",
                rusqlite::params![delta, player_id, row.local_date.as_str()],
            )
            .map_err(|source| TurnError::Sqlite { source })?;

        // Zero rows updated ⇒ the guard rejected the spend. The row
        // exists (ensure_today_turns just materialised it), so a
        // miss can only mean `balance < amount`. Re-read the
        // canonical balance for the error payload rather than
        // trusting `row.balance` — a sibling spend could have landed
        // between ensure and update, and we want the error to report
        // what the next caller will actually see.
        if updated == 0 {
            let current_balance: i64 = self
                .connection()
                .query_row(
                    "SELECT balance FROM turn_ledger \
                     WHERE player_id = ?1 AND local_date = ?2",
                    rusqlite::params![player_id, row.local_date.as_str()],
                    |r| r.get(0),
                )
                .map_err(|source| TurnError::Sqlite { source })?;
            return Err(TurnError::InsufficientTurns {
                player_id,
                balance: current_balance,
                requested: delta,
            });
        }

        // Read back the row so the caller sees the post-spend
        // balance. The stored `daily_allowance` is unchanged by the
        // spend; we re-read it anyway so the returned struct is a
        // straightforward "what's in the DB right now" snapshot.
        let (balance, stored_allowance): (i64, i64) = self
            .connection()
            .query_row(
                "SELECT balance, daily_allowance FROM turn_ledger \
                 WHERE player_id = ?1 AND local_date = ?2",
                rusqlite::params![player_id, row.local_date.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|source| TurnError::Sqlite { source })?;

        Ok(TurnLedgerRow {
            player_id,
            local_date: row.local_date,
            balance,
            daily_allowance: stored_allowance,
        })
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
            let TurnError::InvalidLocalDate { input: got } = &err else {
                panic!("expected InvalidLocalDate variant, got {err:?}");
            };
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

    /// Stand up a fresh world DB with the players + turn_ledger
    /// migrations already applied and one seeded player. Centralises
    /// the boilerplate the Task 6c–6f tests share so the assertions
    /// in each test stay focused on the behavior under test rather
    /// than the setup ceremony.
    fn world_with_player(dir: &tempfile::TempDir) -> (WorldDb, i64) {
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&TURN_LEDGER_MIGRATION)
            .expect("turn_ledger migration applies");
        // Seed a player directly via SQL — the players module's
        // upsert path is covered by Task 5 tests; here we only need
        // a stable id to attach the ledger to.
        world
            .connection()
            .execute("INSERT INTO players (id, handle) VALUES (1, 'alice')", [])
            .expect("seed player");
        (world, 1)
    }

    /// SPEC_v2 §Task 6c headline: a player who has no row for today
    /// receives one with `balance == daily_allowance`. The test seeds
    /// only the schema and a player record — no ledger row — and
    /// asserts the first call materialises the row at the configured
    /// allowance.
    #[test]
    fn ensure_today_turns_creates_row_at_configured_allowance_for_new_player() {
        let dir = tempdir().expect("tempdir creates");
        let (world, player_id) = world_with_player(&dir);
        let provider =
            FixedDateProvider::new(LocalDate::parse("2026-05-08").expect("fixture date parses"));

        let row = world
            .ensure_today_turns(player_id, 30, &provider)
            .expect("first ensure creates the row");

        assert_eq!(row.player_id, player_id);
        assert_eq!(row.local_date.as_str(), "2026-05-08");
        assert_eq!(
            row.balance, 30,
            "new player's balance must equal the configured daily allowance"
        );
        assert_eq!(
            row.daily_allowance, 30,
            "stored allowance snapshot must match the value passed in"
        );

        // Belt-and-braces: the row really exists in the DB, not just
        // in the returned struct. A regression that returned a
        // synthesised row without persisting it would slip past the
        // struct-only assertion above.
        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM turn_ledger \
                 WHERE player_id = ?1 AND local_date = ?2",
                rusqlite::params![player_id, "2026-05-08"],
                |row| row.get(0),
            )
            .expect("count query runs");
        assert_eq!(count, 1, "exactly one ledger row must be persisted");
    }

    /// Calling `ensure_today_turns` twice on the same `(player,
    /// date)` is a no-op on the second call — the existing balance
    /// is preserved verbatim. This is the contract Task 6d's spend
    /// path relies on: a render that calls `ensure_today_turns` to
    /// display "remaining turns" must not undo a spend that landed
    /// earlier in the same day.
    ///
    /// We simulate "a spend already happened" by writing a lower
    /// balance directly via SQL after the first ensure, then call
    /// ensure again and assert the lower balance is what comes back.
    #[test]
    fn ensure_today_turns_is_idempotent_and_preserves_existing_balance() {
        let dir = tempdir().expect("tempdir creates");
        let (world, player_id) = world_with_player(&dir);
        let provider =
            FixedDateProvider::new(LocalDate::parse("2026-05-08").expect("fixture date parses"));

        let first = world
            .ensure_today_turns(player_id, 30, &provider)
            .expect("first ensure creates the row");
        assert_eq!(first.balance, 30);

        // Stand in for Task 6d's spend: drop the balance directly.
        world
            .connection()
            .execute(
                "UPDATE turn_ledger SET balance = ?1 \
                 WHERE player_id = ?2 AND local_date = ?3",
                rusqlite::params![25_i64, player_id, "2026-05-08"],
            )
            .expect("simulated spend");

        let second = world
            .ensure_today_turns(player_id, 30, &provider)
            .expect("second ensure is a no-op");
        assert_eq!(
            second.balance, 25,
            "ensure must not reset a balance that's already been spent down"
        );
        assert_eq!(
            second.daily_allowance, 30,
            "stored allowance is preserved across repeat calls"
        );
    }

    /// Two distinct players with the same date get two distinct
    /// rows. Without this the composite primary key would behave like
    /// a single-row cache and one player's balance would clobber
    /// another's.
    #[test]
    fn ensure_today_turns_creates_separate_rows_per_player() {
        let dir = tempdir().expect("tempdir creates");
        let (world, alice_id) = world_with_player(&dir);
        // Add a second player so the second ensure has a valid id.
        world
            .connection()
            .execute("INSERT INTO players (id, handle) VALUES (2, 'bob')", [])
            .expect("seed second player");
        let bob_id = 2_i64;

        let provider =
            FixedDateProvider::new(LocalDate::parse("2026-05-08").expect("fixture date parses"));

        let alice = world
            .ensure_today_turns(alice_id, 30, &provider)
            .expect("alice row");
        let bob = world
            .ensure_today_turns(bob_id, 30, &provider)
            .expect("bob row");

        assert_eq!(alice.player_id, alice_id);
        assert_eq!(bob.player_id, bob_id);
        assert_eq!(alice.balance, 30);
        assert_eq!(bob.balance, 30);

        let total: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM turn_ledger", [], |row| row.get(0))
            .expect("count query runs");
        assert_eq!(total, 2, "each (player, date) pair must occupy its own row");
    }

    /// Same player, two different dates → two rows. Confirms the
    /// composite primary key actually keys on the date and that
    /// advancing the [`FixedDateProvider`] yields a fresh row at the
    /// new date — the bedrock Task 6f's reset behavior will build on.
    #[test]
    fn ensure_today_turns_creates_separate_rows_per_date() {
        let dir = tempdir().expect("tempdir creates");
        let (world, player_id) = world_with_player(&dir);
        let mut provider =
            FixedDateProvider::new(LocalDate::parse("2026-05-08").expect("first date parses"));

        let day_one = world
            .ensure_today_turns(player_id, 30, &provider)
            .expect("day one row");
        assert_eq!(day_one.local_date.as_str(), "2026-05-08");
        assert_eq!(day_one.balance, 30);

        provider.set(LocalDate::parse("2026-05-09").expect("second date parses"));
        let day_two = world
            .ensure_today_turns(player_id, 30, &provider)
            .expect("day two row");
        assert_eq!(day_two.local_date.as_str(), "2026-05-09");
        assert_eq!(
            day_two.balance, 30,
            "Task 6c writes a fresh allowance for the new date — \
             carryover (Task 6f) is a separate concern"
        );

        let total: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM turn_ledger WHERE player_id = ?1",
                rusqlite::params![player_id],
                |row| row.get(0),
            )
            .expect("count query runs");
        assert_eq!(total, 2, "one row per (player, date) pair");
    }

    /// SQLite errors propagate as [`TurnError::Sqlite`]. We force a
    /// failure by skipping the `turn_ledger` migration entirely so
    /// the INSERT hits a missing-table error. A regression that
    /// `unwrap()`ed on the `rusqlite::Error` would panic instead of
    /// returning a typed error, which the operator-facing layer
    /// (Task 10) wouldn't be able to wrap into `anyhow`.
    #[test]
    fn ensure_today_turns_returns_typed_sqlite_error_on_missing_table() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let world = WorldDb::open(&db_path).expect("open succeeds");
        // Deliberately do NOT apply the turn_ledger migration.
        let provider =
            FixedDateProvider::new(LocalDate::parse("2026-05-08").expect("fixture date parses"));

        let err = world
            .ensure_today_turns(1, 30, &provider)
            .expect_err("missing turn_ledger table must surface as a typed error");
        let TurnError::Sqlite { source } = &err else {
            panic!("expected Sqlite variant, got {err:?}");
        };
        let msg = source.to_string().to_lowercase();
        assert!(
            msg.contains("turn_ledger") || msg.contains("no such table"),
            "underlying SQLite error should mention the missing table, got: {msg}"
        );
    }

    /// SPEC_v2 §Task 6d headline: spending decrements today's balance.
    /// The first call materialises today's row at the configured
    /// allowance (composing with Task 6c) and then applies the
    /// decrement; the returned [`TurnLedgerRow`] reflects the post-
    /// spend balance. A regression that forgot to call the UPDATE
    /// (or ran it against the wrong row) would leave the balance at
    /// `daily_allowance` and flunk this assertion.
    #[test]
    fn spend_turns_decrements_balance_for_existing_player() {
        let dir = tempdir().expect("tempdir creates");
        let (world, player_id) = world_with_player(&dir);
        let provider =
            FixedDateProvider::new(LocalDate::parse("2026-05-08").expect("fixture date parses"));

        let after = world
            .spend_turns(player_id, 1, 30, &provider)
            .expect("first spend succeeds");

        assert_eq!(after.player_id, player_id);
        assert_eq!(after.local_date.as_str(), "2026-05-08");
        assert_eq!(
            after.balance, 29,
            "balance must drop by the spent amount (30 - 1)"
        );
        assert_eq!(
            after.daily_allowance, 30,
            "stored allowance is unchanged by a spend"
        );

        // Belt-and-braces: the persisted row matches the returned
        // snapshot. A regression that returned a synthesised
        // post-spend struct without writing the UPDATE would slip
        // past the struct-only assertion above.
        let persisted: i64 = world
            .connection()
            .query_row(
                "SELECT balance FROM turn_ledger \
                 WHERE player_id = ?1 AND local_date = ?2",
                rusqlite::params![player_id, "2026-05-08"],
                |row| row.get(0),
            )
            .expect("balance readback");
        assert_eq!(persisted, 29, "DB row must reflect the decrement");
    }

    /// Repeated spends compose: balance after N spends of `amount`
    /// is `daily_allowance - N * amount`. This pins the atomicity
    /// contract — each UPDATE applies to the *current* balance, not
    /// to a stale snapshot the method captured at entry.
    #[test]
    fn spend_turns_applies_repeatedly_against_current_balance() {
        let dir = tempdir().expect("tempdir creates");
        let (world, player_id) = world_with_player(&dir);
        let provider =
            FixedDateProvider::new(LocalDate::parse("2026-05-08").expect("fixture date parses"));

        let first = world
            .spend_turns(player_id, 5, 30, &provider)
            .expect("first spend");
        assert_eq!(first.balance, 25);

        let second = world
            .spend_turns(player_id, 5, 30, &provider)
            .expect("second spend");
        assert_eq!(
            second.balance, 20,
            "second spend must apply to the post-first balance, not the original allowance"
        );

        let third = world
            .spend_turns(player_id, 7, 30, &provider)
            .expect("third spend");
        assert_eq!(third.balance, 13, "30 - 5 - 5 - 7 = 13");
    }

    /// Spending an amount of zero is a no-op on the balance — the
    /// row is materialised if missing, but the UPDATE leaves the
    /// counter alone. We don't reject it as an error because Task 6e
    /// will own the typed-error story for "spend can't proceed";
    /// for 6d we just need to confirm the decrement formula
    /// (`balance - 0 == balance`) doesn't accidentally clobber the
    /// row to some other value.
    #[test]
    fn spend_turns_zero_amount_leaves_balance_unchanged() {
        let dir = tempdir().expect("tempdir creates");
        let (world, player_id) = world_with_player(&dir);
        let provider =
            FixedDateProvider::new(LocalDate::parse("2026-05-08").expect("fixture date parses"));

        let row = world
            .spend_turns(player_id, 0, 30, &provider)
            .expect("zero-amount spend is a successful no-op");

        assert_eq!(row.balance, 30, "balance unchanged by a zero-amount spend");
        assert_eq!(row.daily_allowance, 30);
    }

    /// One player's spend does not affect another player's balance.
    /// A regression that dropped the `WHERE player_id = ?` clause
    /// from the UPDATE would decrement every row and flunk here.
    #[test]
    fn spend_turns_isolates_balances_per_player() {
        let dir = tempdir().expect("tempdir creates");
        let (world, alice_id) = world_with_player(&dir);
        world
            .connection()
            .execute("INSERT INTO players (id, handle) VALUES (2, 'bob')", [])
            .expect("seed second player");
        let bob_id = 2_i64;
        let provider =
            FixedDateProvider::new(LocalDate::parse("2026-05-08").expect("fixture date parses"));

        // Materialise Bob's row first so we can assert it is left
        // untouched by Alice's spend.
        let bob_before = world
            .ensure_today_turns(bob_id, 30, &provider)
            .expect("bob row");
        assert_eq!(bob_before.balance, 30);

        let alice_after = world
            .spend_turns(alice_id, 4, 30, &provider)
            .expect("alice spend");
        assert_eq!(alice_after.balance, 26);

        let bob_after = world
            .ensure_today_turns(bob_id, 30, &provider)
            .expect("bob row readback");
        assert_eq!(
            bob_after.balance, 30,
            "alice's spend must not touch bob's balance"
        );
    }

    /// `spend_turns` calls today's lazy-init path internally, so a
    /// brand-new player whose ledger row does not yet exist still gets
    /// a correct post-spend balance. This is the shape callers in
    /// Murder Motel (Task 13a) will use: they spend on the first
    /// inspection of the day without first calling `ensure_today_turns`
    /// themselves.
    #[test]
    fn spend_turns_creates_today_row_lazily_then_decrements() {
        let dir = tempdir().expect("tempdir creates");
        let (world, player_id) = world_with_player(&dir);
        let provider =
            FixedDateProvider::new(LocalDate::parse("2026-05-08").expect("fixture date parses"));

        // Sanity: no ledger row exists yet for this player/day.
        let before: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM turn_ledger \
                 WHERE player_id = ?1 AND local_date = ?2",
                rusqlite::params![player_id, "2026-05-08"],
                |row| row.get(0),
            )
            .expect("count pre-spend");
        assert_eq!(before, 0, "precondition: no ledger row yet");

        let after = world
            .spend_turns(player_id, 3, 30, &provider)
            .expect("lazy-init spend");
        assert_eq!(after.balance, 27, "30 (lazy) - 3 (spent) = 27");

        let after_count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM turn_ledger \
                 WHERE player_id = ?1 AND local_date = ?2",
                rusqlite::params![player_id, "2026-05-08"],
                |row| row.get(0),
            )
            .expect("count post-spend");
        assert_eq!(after_count, 1, "lazy init must persist exactly one row");
    }

    /// A SQLite failure during spend (here forced by skipping the
    /// `turn_ledger` migration) surfaces as a typed
    /// [`TurnError::Sqlite`] rather than a panic. This is the
    /// contract Task 10's operator-facing layer relies on to wrap
    /// world errors into `anyhow` without losing the underlying
    /// SQLite reason.
    #[test]
    fn spend_turns_returns_typed_sqlite_error_on_missing_table() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let world = WorldDb::open(&db_path).expect("open succeeds");
        // Deliberately skip the turn_ledger migration so the inner
        // ensure_today_turns call hits a missing-table error.
        let provider =
            FixedDateProvider::new(LocalDate::parse("2026-05-08").expect("fixture date parses"));

        let err = world
            .spend_turns(1, 1, 30, &provider)
            .expect_err("missing turn_ledger table must surface as a typed error");
        let TurnError::Sqlite { source } = &err else {
            panic!("expected Sqlite variant, got {err:?}");
        };
        let msg = source.to_string().to_lowercase();
        assert!(
            msg.contains("turn_ledger") || msg.contains("no such table"),
            "underlying SQLite error should mention the missing table, got: {msg}"
        );
    }

    /// SPEC_v2 §Task 6e headline: a spend that exceeds today's
    /// balance is rejected as [`TurnError::InsufficientTurns`] **and**
    /// the persisted row is left untouched. The test spends the
    /// allowance most of the way down, then asks for more than is
    /// left; we assert both the typed error variant and the unchanged
    /// DB balance. A regression that decremented past zero (the
    /// pre-6e behavior) would change the persisted row and flunk the
    /// readback assertion.
    #[test]
    fn spend_turns_rejects_insufficient_turns_without_changing_balance() {
        let dir = tempdir().expect("tempdir creates");
        let (world, player_id) = world_with_player(&dir);
        let provider =
            FixedDateProvider::new(LocalDate::parse("2026-05-08").expect("fixture date parses"));

        // Burn most of the daily allowance via the public API so the
        // setup goes through the same code path real callers use.
        // Allowance = 5, spent = 4 → balance = 1 going into the
        // attempted over-spend.
        let pre = world
            .spend_turns(player_id, 4, 5, &provider)
            .expect("setup spend succeeds");
        assert_eq!(
            pre.balance, 1,
            "precondition: one turn left before over-spend"
        );

        let err = world
            .spend_turns(player_id, 2, 5, &provider)
            .expect_err("over-spend must be rejected");
        match err {
            TurnError::InsufficientTurns {
                player_id: pid,
                balance,
                requested,
            } => {
                assert_eq!(pid, player_id);
                assert_eq!(balance, 1, "error must report the actual remaining balance");
                assert_eq!(requested, 2, "error must echo the rejected request size");
            }
            other => panic!("expected InsufficientTurns, got {other:?}"),
        }

        // The persisted row must be untouched by the rejected spend.
        // Reading it back via SQL (rather than another spend_turns
        // call) keeps the assertion focused on the persistence
        // contract and avoids any chance the readback path papers
        // over a half-applied UPDATE.
        let persisted: i64 = world
            .connection()
            .query_row(
                "SELECT balance FROM turn_ledger \
                 WHERE player_id = ?1 AND local_date = ?2",
                rusqlite::params![player_id, "2026-05-08"],
                |row| row.get(0),
            )
            .expect("balance readback");
        assert_eq!(
            persisted, 1,
            "persisted balance must be unchanged when InsufficientTurns is returned"
        );
    }

    /// A spend that exactly equals the remaining balance succeeds and
    /// drives the balance to zero. This pins the off-by-one corner of
    /// the `WHERE balance >= ?` guard: `>=` (not `>`) is what allows
    /// the player to spend the very last turn. A regression that
    /// tightened the guard to `>` would reject this spend and flunk
    /// here.
    #[test]
    fn spend_turns_allows_exact_balance_spend_to_zero() {
        let dir = tempdir().expect("tempdir creates");
        let (world, player_id) = world_with_player(&dir);
        let provider =
            FixedDateProvider::new(LocalDate::parse("2026-05-08").expect("fixture date parses"));

        let after = world
            .spend_turns(player_id, 5, 5, &provider)
            .expect("spending the exact balance must succeed");
        assert_eq!(
            after.balance, 0,
            "exact-balance spend must drive the balance to zero"
        );

        // A subsequent spend of any positive amount with balance == 0
        // must be rejected — confirms the guard still works at the
        // boundary on the next call.
        let err = world
            .spend_turns(player_id, 1, 5, &provider)
            .expect_err("any positive spend at zero balance must be rejected");
        assert!(
            matches!(err, TurnError::InsufficientTurns { balance: 0, .. }),
            "expected InsufficientTurns at balance 0, got {err:?}"
        );
    }

    /// Zero-amount spend at zero balance is still a no-op success.
    /// `balance >= 0` is trivially true, so the existing Task 6d
    /// "zero amount leaves balance unchanged" contract continues to
    /// hold even after the 6e guard is in place. Without this test a
    /// future tightening of the guard (e.g. `balance >= ?1 AND ?1 >
    /// 0`) could silently break the no-op contract.
    #[test]
    fn spend_turns_zero_amount_at_zero_balance_remains_a_no_op() {
        let dir = tempdir().expect("tempdir creates");
        let (world, player_id) = world_with_player(&dir);
        let provider =
            FixedDateProvider::new(LocalDate::parse("2026-05-08").expect("fixture date parses"));

        // Drain the balance to exactly zero first.
        let drained = world
            .spend_turns(player_id, 5, 5, &provider)
            .expect("drain to zero");
        assert_eq!(drained.balance, 0);

        let again = world
            .spend_turns(player_id, 0, 5, &provider)
            .expect("zero-amount spend at zero balance must still succeed");
        assert_eq!(again.balance, 0);
    }
}
