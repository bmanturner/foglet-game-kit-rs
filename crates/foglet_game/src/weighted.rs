//! Deterministic weighted-selection helpers for game-authored tables.
//!
//! The kit intentionally stays out of game content formats. A game can
//! store tables as Rust constants, TOML, YAML, JSON, or rows in its own
//! SQLite schema, then pass the stable keys and non-negative weights to
//! this module when it needs one deterministic pick.
//!
//! # Stable keys
//!
//! Use stable machine keys, not display text. Good examples:
//!
//! - `"travel.clear"`
//! - `"tick.market-shortage"`
//! - `"npc.merchant-rumor"`
//!
//! These keys can be shown in explanation output, stored in logs, or
//! mapped back to game-local content. The kit does not parse them.

/// One game-authored weighted entry.
///
/// `key` should be a stable game-owned identifier. `weight` is `u64`
/// so callers cannot express negative weights. A zero weight is valid
/// input, but it is filtered out of selection and reported as
/// [`WeightedRejectedReason::ZeroWeight`] by
/// [`explain_weighted_selection`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeightedEntry<K> {
    /// Stable game-authored key for the selected outcome.
    pub key: K,
    /// Non-negative relative weight. Zero means "not selectable now".
    pub weight: u64,
}

impl<K> WeightedEntry<K> {
    /// Build one weighted table entry.
    pub fn new(key: K, weight: u64) -> Self {
        Self { key, weight }
    }
}

/// Result of mapping a seed into a non-empty weighted range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeightedRoll {
    /// Seed string supplied by the caller.
    pub seed: String,
    /// Stable FNV-1a hash of [`WeightedRoll::seed`].
    pub hash: u64,
    /// Sum of selectable entry weights.
    pub total_weight: u128,
    /// Zero-based bucket inside `0..total_weight`.
    pub bucket: u128,
}

/// Why no selectable key could be returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeightedNoSelectionReason {
    /// The caller supplied no entries at all.
    EmptyTable,
    /// Entries existed, but every entry had zero weight.
    ZeroTotalWeight,
}

/// Non-panicking selection result returned by [`select_weighted`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeightedSelection<'a, K> {
    /// Borrowed key of the selected entry, or `None` when the table has
    /// no selectable entries.
    pub selected_key: Option<&'a K>,
    /// Stable hash of the supplied seed.
    pub hash: u64,
    /// Sum of weights that were eligible for selection.
    pub total_weight: u128,
    /// Zero-based roll bucket. `None` when `total_weight == 0`.
    pub bucket: Option<u128>,
    /// Explicit no-selection reason for empty or zero-total tables.
    pub no_selection_reason: Option<WeightedNoSelectionReason>,
}

/// Rejected entry included in explanation output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeightedRejectedEntry {
    /// Stable key of the rejected entry.
    pub key: String,
    /// Reason this entry was not part of the selectable total.
    pub reason: WeightedRejectedReason,
}

/// Reason an entry was filtered out before rolling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeightedRejectedReason {
    /// Zero-weight entries remain visible to explainers, but they
    /// cannot be selected.
    ZeroWeight,
}

/// Allocating, diagnostics-friendly explanation of a weighted roll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeightedSelectionExplanation {
    /// Seed string supplied by the caller.
    pub seed: String,
    /// Stable hash of [`WeightedSelectionExplanation::seed`].
    pub hash: u64,
    /// Sum of selectable entry weights.
    pub total_weight: u128,
    /// Zero-based bucket inside `0..total_weight`, or `None` for empty
    /// and zero-total tables.
    pub bucket: Option<u128>,
    /// Stable key that won the roll, or `None` when no selection was
    /// possible.
    pub selected_key: Option<String>,
    /// Filtered entries and their reasons.
    pub rejected: Vec<WeightedRejectedEntry>,
    /// Explicit no-selection reason for empty or zero-total tables.
    pub no_selection_reason: Option<WeightedNoSelectionReason>,
}

/// Compute a stable 64-bit FNV-1a hash for `seed`.
///
/// This helper is intentionally independent of Rust's standard hash
/// traits because `DefaultHasher` does not promise stable values across
/// compiler releases. FNV-1a is small, deterministic, and sufficient
/// for mapping game-authored seeds onto weighted buckets.
pub fn stable_hash(seed: &str) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    for byte in seed.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// Map `seed` into `0..total_weight`.
///
/// Returns `None` when `total_weight == 0`, which lets callers keep
/// zero-total table handling explicit instead of accidentally treating
/// bucket zero as meaningful.
pub fn seed_to_bucket(seed: &str, total_weight: u128) -> Option<WeightedRoll> {
    if total_weight == 0 {
        return None;
    }

    let hash = stable_hash(seed);
    Some(WeightedRoll {
        seed: seed.to_string(),
        hash,
        total_weight,
        bucket: u128::from(hash) % total_weight,
    })
}

/// Select one key from `entries` using `seed`.
///
/// The same seed and same entry list always produce the same selected
/// key. Entries with zero weight are ignored. Empty tables and
/// zero-total tables return `selected_key: None` plus an explicit
/// [`WeightedNoSelectionReason`].
pub fn select_weighted<'a, K: AsRef<str>>(
    seed: &str,
    entries: &'a [WeightedEntry<K>],
) -> WeightedSelection<'a, K> {
    let hash = stable_hash(seed);
    let total_weight = selectable_total(entries);

    if total_weight == 0 {
        return WeightedSelection {
            selected_key: None,
            hash,
            total_weight,
            bucket: None,
            no_selection_reason: Some(if entries.is_empty() {
                WeightedNoSelectionReason::EmptyTable
            } else {
                WeightedNoSelectionReason::ZeroTotalWeight
            }),
        };
    }

    let bucket = u128::from(hash) % total_weight;
    let mut cursor = 0_u128;
    for entry in entries {
        if entry.weight == 0 {
            continue;
        }

        cursor += u128::from(entry.weight);
        if bucket < cursor {
            return WeightedSelection {
                selected_key: Some(&entry.key),
                hash,
                total_weight,
                bucket: Some(bucket),
                no_selection_reason: None,
            };
        }
    }

    // `bucket < total_weight` and `cursor == total_weight`, so the loop
    // above should always return. Keep a defensive explicit failure
    // rather than panicking if a future edit breaks that invariant.
    WeightedSelection {
        selected_key: None,
        hash,
        total_weight,
        bucket: Some(bucket),
        no_selection_reason: Some(WeightedNoSelectionReason::ZeroTotalWeight),
    }
}

/// Select one key and return a diagnostics-friendly explanation.
///
/// This is the allocation-bearing companion to [`select_weighted`].
/// It captures the seed, hash, total weight, bucket, selected key, and
/// rejected zero-weight entries so tests, debug screens, and operator
/// logs can explain why a procedural pick happened.
pub fn explain_weighted_selection<K: AsRef<str>>(
    seed: &str,
    entries: &[WeightedEntry<K>],
) -> WeightedSelectionExplanation {
    let selection = select_weighted(seed, entries);
    let rejected = entries
        .iter()
        .filter(|entry| entry.weight == 0)
        .map(|entry| WeightedRejectedEntry {
            key: entry.key.as_ref().to_string(),
            reason: WeightedRejectedReason::ZeroWeight,
        })
        .collect();

    WeightedSelectionExplanation {
        seed: seed.to_string(),
        hash: selection.hash,
        total_weight: selection.total_weight,
        bucket: selection.bucket,
        selected_key: selection.selected_key.map(|key| key.as_ref().to_string()),
        rejected,
        no_selection_reason: selection.no_selection_reason,
    }
}

fn selectable_total<K>(entries: &[WeightedEntry<K>]) -> u128 {
    entries
        .iter()
        .map(|entry| u128::from(entry.weight))
        .sum::<u128>()
}

#[cfg(test)]
mod tests {
    use super::{
        explain_weighted_selection, seed_to_bucket, select_weighted, stable_hash, WeightedEntry,
        WeightedNoSelectionReason, WeightedRejectedEntry, WeightedRejectedReason,
    };

    #[test]
    fn selection_is_repeatable_for_same_seed_and_entries() {
        let entries = [
            WeightedEntry::new("travel.clear", 3),
            WeightedEntry::new("travel.delay", 2),
            WeightedEntry::new("travel.encounter", 1),
        ];

        let first = select_weighted("player-7:route-3:2026-05-13", &entries);
        let second = select_weighted("player-7:route-3:2026-05-13", &entries);

        assert_eq!(first, second);
        assert!(
            first.selected_key.is_some(),
            "positive-weight table should select one entry"
        );
    }

    #[test]
    fn selection_is_sensitive_to_seed_changes() {
        let entries = [
            WeightedEntry::new("tick.quiet", 1),
            WeightedEntry::new("tick.shortage", 1),
            WeightedEntry::new("tick.windfall", 1),
        ];

        let baseline = select_weighted("world-tick:0", &entries)
            .selected_key
            .copied()
            .expect("baseline selects");

        let changed = (1..64)
            .map(|n| format!("world-tick:{n}"))
            .find_map(|seed| {
                let selected = select_weighted(&seed, &entries).selected_key.copied()?;
                (selected != baseline).then_some(selected)
            })
            .expect("nearby seeds should reach another equal-weight bucket");

        assert_ne!(baseline, changed);
        assert_ne!(stable_hash("world-tick:0"), stable_hash("world-tick:1"));
    }

    #[test]
    fn empty_and_zero_weight_tables_return_explicit_no_selection() {
        let empty: [WeightedEntry<&str>; 0] = [];
        let empty_selection = select_weighted("npc:rumor", &empty);
        assert_eq!(empty_selection.selected_key, None);
        assert_eq!(empty_selection.total_weight, 0);
        assert_eq!(empty_selection.bucket, None);
        assert_eq!(
            empty_selection.no_selection_reason,
            Some(WeightedNoSelectionReason::EmptyTable)
        );
        assert_eq!(seed_to_bucket("npc:rumor", 0), None);

        let zero = [
            WeightedEntry::new("npc.trade-tip", 0),
            WeightedEntry::new("npc.local-warning", 0),
        ];
        let zero_selection = select_weighted("npc:rumor", &zero);
        assert_eq!(zero_selection.selected_key, None);
        assert_eq!(zero_selection.total_weight, 0);
        assert_eq!(
            zero_selection.no_selection_reason,
            Some(WeightedNoSelectionReason::ZeroTotalWeight)
        );
    }

    #[test]
    fn explanation_reports_roll_and_filtered_entries() {
        let entries = [
            WeightedEntry::new("npc.greeting", 2),
            WeightedEntry::new("npc.closed-shop", 0),
            WeightedEntry::new("npc.rumor", 5),
        ];

        let explanation = explain_weighted_selection("npc:daily:17", &entries);

        assert_eq!(explanation.seed, "npc:daily:17");
        assert_eq!(explanation.hash, stable_hash("npc:daily:17"));
        assert_eq!(explanation.total_weight, 7);
        assert!(
            explanation.bucket.is_some_and(|bucket| bucket < 7),
            "bucket should be inside total weight"
        );
        assert!(
            matches!(
                explanation.selected_key.as_deref(),
                Some("npc.greeting" | "npc.rumor")
            ),
            "zero-weight entry should never be selected"
        );
        assert_eq!(
            explanation.rejected,
            vec![WeightedRejectedEntry {
                key: "npc.closed-shop".to_string(),
                reason: WeightedRejectedReason::ZeroWeight,
            }]
        );
        assert_eq!(explanation.no_selection_reason, None);
    }
}
