//! Multi-user local-dev test harness.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use thiserror::Error;

use crate::config::{
    GameConfig, GameSection, ManifestSection, MultiplayerSection, SaveSection, SaveStrategy,
    WorldSection,
};
use crate::events::{EventError, EventRecord, WORLD_EVENTS_MIGRATION};
use crate::foglet::{ContextSource, FogletContext};
use crate::notices::{Notice, NoticeError, NOTICES_MIGRATION};
use crate::players::{PlayerError, PLAYERS_MIGRATION};
use crate::roles::FogletRole;
use crate::screen::GameContext;
use crate::world_db::{WorldDb, WorldDbError};

/// Test-only harness with one shared world DB and per-user roots.
#[derive(Debug)]
pub struct MultiUserHarness {
    tempdir: TempDir,
    world_db_path: PathBuf,
    world_db: WorldDb,
    config: GameConfig,
    users: HashMap<String, HarnessUser>,
    notices_enabled: bool,
}

/// Builder for [`MultiUserHarness`].
#[derive(Debug, Default)]
pub struct MultiUserHarnessBuilder {
    users: Vec<UserSpec>,
    notices_enabled: bool,
}

#[derive(Debug, Clone)]
struct UserSpec {
    handle: String,
    role: FogletRole,
}

#[derive(Debug, Clone)]
struct HarnessUser {
    foglet: FogletContext,
    save_root: PathBuf,
    player_id: i64,
}

/// Errors produced while building or querying a multi-user harness.
#[derive(Debug, Error)]
pub enum MultiUserHarnessError {
    /// The harness needs at least one user.
    #[error("multi-user harness requires at least one user")]
    NoUsers,
    /// A handle was registered more than once.
    #[error("duplicate harness user handle `{handle}`")]
    DuplicateHandle {
        /// Duplicate handle.
        handle: String,
    },
    /// Temp directory creation failed.
    #[error("failed to create multi-user harness temp directory: {source}")]
    TempDir {
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Per-user save root creation failed.
    #[error("failed to create save root for `{handle}` at {path}: {source}")]
    SaveRoot {
        /// User handle.
        handle: String,
        /// Save root path.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Shared world DB open failed.
    #[error("failed to open harness world DB: {source}")]
    WorldDb {
        /// Underlying world DB error.
        #[source]
        source: WorldDbError,
    },
    /// A requested handle is not registered in the harness.
    #[error("unknown harness user handle `{handle}`")]
    UnknownHandle {
        /// Requested handle.
        handle: String,
    },
    /// Harness event visibility assertion failed.
    #[error("no visible event matched for `{handle}`")]
    EventNotVisible {
        /// User handle.
        handle: String,
    },
    /// Reading world events failed.
    #[error("failed to read harness events: {source}")]
    EventRead {
        /// Underlying event error.
        #[source]
        source: EventError,
    },
    /// Applying harness migrations failed.
    #[error("failed to apply harness migration: {source}")]
    Migration {
        /// Underlying world DB error.
        #[source]
        source: WorldDbError,
    },
    /// Seeding a harness player failed.
    #[error("failed to seed harness player: {source}")]
    PlayerSeed {
        /// Underlying player error.
        #[source]
        source: PlayerError,
    },
    /// Notice assertions are unavailable because notices are disabled.
    #[error("harness notices are disabled")]
    NoticesDisabled,
    /// Reading notices failed.
    #[error("failed to read harness notices: {source}")]
    NoticeRead {
        /// Underlying notice error.
        #[source]
        source: NoticeError,
    },
    /// Notice visibility assertion failed.
    #[error("no notice matched for `{handle}`")]
    NoticeNotFound {
        /// User handle.
        handle: String,
    },
}

impl MultiUserHarness {
    /// Start building a multi-user harness.
    #[must_use]
    pub fn builder() -> MultiUserHarnessBuilder {
        MultiUserHarnessBuilder::default()
    }

    /// Shared world DB path used by every harness user.
    #[must_use]
    pub fn world_db_path(&self) -> &Path {
        &self.world_db_path
    }

    /// Shared world DB path as observed by one user handle.
    #[must_use]
    pub fn world_db_path_for(&self, handle: &str) -> Option<&Path> {
        self.users
            .contains_key(handle)
            .then_some(self.world_db_path.as_path())
    }

    /// Per-user save root path.
    #[must_use]
    pub fn save_root_for(&self, handle: &str) -> Option<&Path> {
        self.users.get(handle).map(|user| user.save_root.as_path())
    }

    /// Shared world DB handle.
    #[must_use]
    pub fn world_db(&self) -> &WorldDb {
        &self.world_db
    }

    /// Mutable shared world DB handle.
    pub fn world_db_mut(&mut self) -> &mut WorldDb {
        &mut self.world_db
    }

    /// Shared config used by harness contexts.
    #[must_use]
    pub fn config(&self) -> &GameConfig {
        &self.config
    }

    /// Foglet context for one user.
    #[must_use]
    pub fn foglet_for(&self, handle: &str) -> Option<&FogletContext> {
        self.users.get(handle).map(|user| &user.foglet)
    }

    /// Player id for one harness user.
    #[must_use]
    pub fn player_id_for(&self, handle: &str) -> Option<i64> {
        self.users.get(handle).map(|user| user.player_id)
    }

    /// Build a [`GameContext`] for one registered user.
    pub fn context_for(&self, handle: &str) -> Result<GameContext<'_>, MultiUserHarnessError> {
        let user = self
            .users
            .get(handle)
            .ok_or_else(|| MultiUserHarnessError::UnknownHandle {
                handle: handle.to_string(),
            })?;
        Ok(GameContext::new(&self.config, &user.foglet, (80, 24)).with_world_db(&self.world_db))
    }

    /// Run a closure with one registered user's [`GameContext`].
    pub fn with_user<R>(
        &self,
        handle: &str,
        f: impl FnOnce(&GameContext<'_>) -> R,
    ) -> Result<R, MultiUserHarnessError> {
        let ctx = self.context_for(handle)?;
        Ok(f(&ctx))
    }

    /// Assert that a visible event exists for one handle.
    pub fn assert_event_visible_to(
        &self,
        handle: &str,
        predicate: impl Fn(&EventRecord) -> bool,
    ) -> Result<(), MultiUserHarnessError> {
        let user = self
            .users
            .get(handle)
            .ok_or_else(|| MultiUserHarnessError::UnknownHandle {
                handle: handle.to_string(),
            })?;
        let events = self
            .world_db
            .recent_events(u32::MAX)
            .map_err(|source| MultiUserHarnessError::EventRead { source })?;
        let matched = events.into_iter().any(|event| {
            (event.player_id.is_none() || event.player_id == Some(user.player_id))
                && predicate(&event)
        });
        matched
            .then_some(())
            .ok_or_else(|| MultiUserHarnessError::EventNotVisible {
                handle: handle.to_string(),
            })
    }

    /// Assert that a matching notice exists for one handle.
    pub fn assert_notice_for(
        &self,
        handle: &str,
        predicate: impl Fn(&Notice) -> bool,
    ) -> Result<(), MultiUserHarnessError> {
        if !self.notices_enabled {
            return Err(MultiUserHarnessError::NoticesDisabled);
        }
        let user = self
            .users
            .get(handle)
            .ok_or_else(|| MultiUserHarnessError::UnknownHandle {
                handle: handle.to_string(),
            })?;
        let notices = self
            .world_db
            .inbox(user.player_id)
            .map_err(|source| MultiUserHarnessError::NoticeRead { source })?;
        notices.iter().any(predicate).then_some(()).ok_or_else(|| {
            MultiUserHarnessError::NoticeNotFound {
                handle: handle.to_string(),
            }
        })
    }

    /// Harness temp root. Exposed for cleanup assertions.
    #[must_use]
    pub fn temp_root(&self) -> &Path {
        self.tempdir.path()
    }
}

impl MultiUserHarnessBuilder {
    /// Add a fake user to the harness.
    #[must_use]
    pub fn add_user(mut self, handle: impl Into<String>, role: FogletRole) -> Self {
        self.users.push(UserSpec {
            handle: handle.into(),
            role,
        });
        self
    }

    /// Enable notice helpers for the harness.
    #[must_use]
    pub fn with_notices_enabled(mut self) -> Self {
        self.notices_enabled = true;
        self
    }

    /// Build the harness.
    pub fn build(self) -> Result<MultiUserHarness, MultiUserHarnessError> {
        if self.users.is_empty() {
            return Err(MultiUserHarnessError::NoUsers);
        }

        let tempdir =
            tempfile::tempdir().map_err(|source| MultiUserHarnessError::TempDir { source })?;
        let world_db_path = tempdir.path().join("world").join("world.sqlite");
        let mut world_db = WorldDb::open(&world_db_path)
            .map_err(|source| MultiUserHarnessError::WorldDb { source })?;
        world_db
            .apply_migration(&PLAYERS_MIGRATION)
            .map_err(|source| MultiUserHarnessError::Migration { source })?;
        world_db
            .apply_migration(&WORLD_EVENTS_MIGRATION)
            .map_err(|source| MultiUserHarnessError::Migration { source })?;
        if self.notices_enabled {
            world_db
                .apply_migration(&NOTICES_MIGRATION)
                .map_err(|source| MultiUserHarnessError::Migration { source })?;
        }
        let mut users = HashMap::new();

        for spec in self.users {
            if users.contains_key(&spec.handle) {
                return Err(MultiUserHarnessError::DuplicateHandle {
                    handle: spec.handle,
                });
            }
            let save_root = tempdir.path().join("saves").join(&spec.handle);
            std::fs::create_dir_all(&save_root).map_err(|source| {
                MultiUserHarnessError::SaveRoot {
                    handle: spec.handle.clone(),
                    path: save_root.display().to_string(),
                    source,
                }
            })?;
            let foglet = FogletContext {
                door_id: "multi-user-harness".to_string(),
                user_id: Some(format!("harness:{}", spec.handle)),
                username: Some(spec.handle.clone()),
                role: Some(spec.role.as_token().to_string()),
                session_id: Some(format!("session:{}", spec.handle)),
                terminal_width: 80,
                terminal_height: 24,
                source: ContextSource::LocalDev,
            };
            let player = world_db
                .upsert_player(&foglet)
                .map_err(|source| MultiUserHarnessError::PlayerSeed { source })?;
            users.insert(
                spec.handle,
                HarnessUser {
                    foglet,
                    save_root,
                    player_id: player.id,
                },
            );
        }

        Ok(MultiUserHarness {
            tempdir,
            world_db_path,
            world_db,
            config: harness_config(self.notices_enabled),
            users,
            notices_enabled: self.notices_enabled,
        })
    }
}

fn harness_config(notices_enabled: bool) -> GameConfig {
    GameConfig {
        game: GameSection {
            title: "Multi User Harness".into(),
            slug: "multi-user-harness".into(),
            description: "test harness".into(),
            min_width: 80,
            min_height: 24,
            start_map: "harness".into(),
            start_x: 0,
            start_y: 0,
        },
        save: SaveSection {
            strategy: SaveStrategy::PerFogletUser,
        },
        manifest: ManifestSection {
            timeout_ms: 1_800_000,
            idle_timeout_ms: 300_000,
            visibility: "members".into(),
            auth_scope: "site".into(),
        },
        world: WorldSection {
            enabled: true,
            ..Default::default()
        },
        turns: None,
        leaderboards: Vec::new(),
        multiplayer: notices_enabled.then(|| MultiplayerSection {
            notices: true,
            ..Default::default()
        }),
        factions: Default::default(),
        spatial: Default::default(),
        presence: Default::default(),
        place_recall: Default::default(),
        inventory: Default::default(),
        world_ticks: Default::default(),
        contracts: Default::default(),
        job_board: Default::default(),
        travel: Default::default(),
        inventory_capacity: Default::default(),
        screens: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::{MultiUserHarness, MultiUserHarnessError};
    use crate::events::append_event_on;
    use crate::inventory::INVENTORY_SLOTS_MIGRATION;
    use crate::inventory_capacity::{CapacityError, CapacityPolicy, FinitePickupResult};
    use crate::place_recall::PLACE_RECALL_MIGRATION;
    use crate::roles::FogletRole;
    use crate::spatial::PLACES_MIGRATION;

    #[test]
    fn builder_creates_one_shared_world_db_and_per_user_save_roots() {
        let harness = MultiUserHarness::builder()
            .add_user("alice", FogletRole::User)
            .add_user("bob", FogletRole::Mod)
            .build()
            .expect("harness builds");

        assert!(harness.world_db_path().ends_with("world/world.sqlite"));
        assert_eq!(
            harness.world_db_path_for("alice"),
            harness.world_db_path_for("bob"),
            "all users share one world DB path"
        );
        assert_ne!(
            harness.save_root_for("alice"),
            harness.save_root_for("bob"),
            "each user gets an isolated save root"
        );
        assert!(harness
            .save_root_for("alice")
            .expect("alice save root")
            .exists());
        assert!(harness
            .save_root_for("bob")
            .expect("bob save root")
            .exists());
    }

    #[test]
    fn builder_rejects_duplicate_handles() {
        let error = MultiUserHarness::builder()
            .add_user("alice", FogletRole::User)
            .add_user("alice", FogletRole::Mod)
            .build()
            .expect_err("duplicate handles should fail");

        match error {
            MultiUserHarnessError::DuplicateHandle { handle } => {
                assert_eq!(handle, "alice");
            }
            other => panic!("expected duplicate handle error, got {other:?}"),
        }
    }

    #[test]
    fn context_for_and_with_user_return_per_handle_contexts_with_isolated_save_roots() {
        let harness = MultiUserHarness::builder()
            .add_user("alice", FogletRole::User)
            .add_user("bob", FogletRole::Sysop)
            .build()
            .expect("harness builds");

        let alice = harness.context_for("alice").expect("alice context");
        let bob_username = harness
            .with_user("bob", |ctx| ctx.foglet.username.clone())
            .expect("bob callback runs");

        assert_eq!(alice.foglet.username.as_deref(), Some("alice"));
        assert_eq!(bob_username.as_deref(), Some("bob"));
        assert!(alice.world_db.is_some());
        assert_ne!(
            harness.save_root_for("alice"),
            harness.save_root_for("bob"),
            "per-user save roots must be isolated"
        );
    }

    #[test]
    fn assert_event_visible_to_distinguishes_player_scoped_and_global_events() {
        let harness = MultiUserHarness::builder()
            .add_user("alice", FogletRole::User)
            .add_user("bob", FogletRole::User)
            .build()
            .expect("harness builds");
        let alice_id = harness.player_id_for("alice").expect("alice player id");
        let bob_id = harness.player_id_for("bob").expect("bob player id");

        harness
            .world_db()
            .append_event("global", None, "all hands", None)
            .expect("global event appends");
        harness
            .world_db()
            .append_event("private", Some(alice_id), "alice note", None)
            .expect("alice event appends");
        harness
            .world_db()
            .append_event("private", Some(bob_id), "bob note", None)
            .expect("bob event appends");

        harness
            .assert_event_visible_to("alice", |event| event.message == "all hands")
            .expect("alice sees global event");
        harness
            .assert_event_visible_to("alice", |event| event.message == "alice note")
            .expect("alice sees her own event");
        assert!(matches!(
            harness.assert_event_visible_to("alice", |event| event.message == "bob note"),
            Err(MultiUserHarnessError::EventNotVisible { handle }) if handle == "alice"
        ));
    }

    #[test]
    fn assert_notice_for_is_gated_and_functional_when_enabled() {
        let disabled = MultiUserHarness::builder()
            .add_user("alice", FogletRole::User)
            .build()
            .expect("disabled harness builds");
        assert!(matches!(
            disabled.assert_notice_for("alice", |_| true),
            Err(MultiUserHarnessError::NoticesDisabled)
        ));

        let enabled = MultiUserHarness::builder()
            .with_notices_enabled()
            .add_user("alice", FogletRole::User)
            .add_user("bob", FogletRole::User)
            .build()
            .expect("enabled harness builds");
        let alice_id = enabled.player_id_for("alice").expect("alice player id");
        let bob_id = enabled.player_id_for("bob").expect("bob player id");
        enabled
            .world_db()
            .send_notice(
                Some(bob_id),
                alice_id,
                "mail",
                "hello",
                "body",
                None,
                None,
                1_000,
            )
            .expect("notice sends");

        enabled
            .assert_notice_for("alice", |notice| notice.subject == "hello")
            .expect("alice notice is visible");
        assert!(matches!(
            enabled.assert_notice_for("bob", |notice| notice.subject == "hello"),
            Err(MultiUserHarnessError::NoticeNotFound { handle }) if handle == "bob"
        ));
    }

    #[test]
    fn drop_cleans_up_temp_world_db_and_save_roots() {
        let harness = MultiUserHarness::builder()
            .add_user("alice", FogletRole::User)
            .add_user("bob", FogletRole::User)
            .build()
            .expect("harness builds");
        let temp_root = harness.temp_root().to_path_buf();
        let world_db_path = harness.world_db_path().to_path_buf();
        let alice_save_root = harness
            .save_root_for("alice")
            .expect("alice save root")
            .to_path_buf();
        let bob_save_root = harness
            .save_root_for("bob")
            .expect("bob save root")
            .to_path_buf();

        assert!(temp_root.exists());
        assert!(world_db_path.exists());
        assert!(alice_save_root.exists());
        assert!(bob_save_root.exists());

        drop(harness);

        assert!(!temp_root.exists());
        assert!(!world_db_path.exists());
        assert!(!alice_save_root.exists());
        assert!(!bob_save_root.exists());
    }

    #[test]
    fn shared_finite_resource_exhaustion_keeps_proof_and_recall_personal() {
        let mut harness = MultiUserHarness::builder()
            .add_user("alice", FogletRole::User)
            .add_user("bob", FogletRole::User)
            .build()
            .expect("harness builds");
        let alice_id = harness.player_id_for("alice").expect("alice player id");
        let bob_id = harness.player_id_for("bob").expect("bob player id");
        let derelict_id;

        {
            let world = harness.world_db_mut();
            world
                .apply_migration(&INVENTORY_SLOTS_MIGRATION)
                .expect("inventory migration applies");
            world
                .apply_migration(&PLACES_MIGRATION)
                .expect("places migration applies");
            world
                .apply_migration(&PLACE_RECALL_MIGRATION)
                .expect("recall migration applies");

            let derelict = world
                .insert_place("derelict-cache", "Derelict Cache", "site", None)
                .expect("derelict place inserts");
            derelict_id = derelict.id;
            world
                .create_slot("site", derelict_id, "black-box", 1, None, None)
                .expect("finite shared source inserts");
            world
                .touch_recall(
                    alice_id,
                    derelict_id,
                    Some(r#"{"personal_note":"entered through the forward lock"}"#),
                )
                .expect("alice personal recall touches");

            let pickup = world
                .take_finite_pickup_with_capacity(
                    ("site", derelict_id),
                    ("player", alice_id),
                    "black-box",
                    1,
                    &UnitCapacity,
                    Some(
                        |tx: &rusqlite::Transaction<'_>,
                         pickup: &FinitePickupResult|
                         -> rusqlite::Result<()> {
                            append_event_on(
                                tx,
                                "salvage_proof",
                                Some(alice_id),
                                "Alice recovered the black box.",
                                Some(r#"{"proof":"wreck-alpha"}"#),
                            )
                            .map_err(|err| {
                                rusqlite::Error::InvalidParameterName(err.to_string())
                            })?;
                            if pickup.source_exhausted {
                                append_event_on(
                                    tx,
                                    "site_exhausted",
                                    None,
                                    "The derelict cache is exhausted.",
                                    Some(r#"{"item":"black-box"}"#),
                                )
                                .map_err(|err| {
                                    rusqlite::Error::InvalidParameterName(err.to_string())
                                })?;
                            }
                            Ok(())
                        },
                    ),
                )
                .expect("alice finite pickup succeeds");
            assert!(pickup.source_exhausted);
        }

        let shared_source = harness
            .world_db()
            .get_slot("site", derelict_id, "black-box")
            .expect("shared source reads")
            .expect("shared source remains as exhausted row");
        assert_eq!(
            shared_source.quantity, 0,
            "Bob can observe the shared finite source is exhausted"
        );
        harness
            .assert_event_visible_to("alice", |event| event.kind == "salvage_proof")
            .expect("alice sees her personal proof event");
        assert!(matches!(
            harness.assert_event_visible_to("bob", |event| event.kind == "salvage_proof"),
            Err(MultiUserHarnessError::EventNotVisible { handle }) if handle == "bob"
        ));
        harness
            .assert_event_visible_to("bob", |event| event.kind == "site_exhausted")
            .expect("bob sees the intentionally shared exhaustion event");

        let alice_recall = harness
            .world_db()
            .recall_for_player(alice_id)
            .expect("alice recall reads");
        assert!(
            alice_recall
                .iter()
                .any(|recall| recall.place_id == derelict_id),
            "Alice keeps her personal recall of the site"
        );
        let bob_recall = harness
            .world_db()
            .recall_for_player(bob_id)
            .expect("bob recall reads");
        assert!(
            bob_recall
                .iter()
                .all(|recall| recall.place_id != derelict_id),
            "Bob's recall remains personal unless the game writes a Bob-scoped recall row"
        );
    }

    struct UnitCapacity;

    impl CapacityPolicy for UnitCapacity {
        fn item_volume(
            &self,
            _item_key: &str,
            _metadata: &serde_json::Value,
        ) -> Result<i64, CapacityError> {
            Ok(1)
        }

        fn owner_capacity(
            &self,
            _owner_kind: &str,
            _owner_id: i64,
        ) -> Result<Option<i64>, CapacityError> {
            Ok(Some(1))
        }
    }
}
