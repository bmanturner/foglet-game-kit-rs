//! Multi-user local-dev test harness.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use thiserror::Error;

use crate::config::{
    GameConfig, GameSection, ManifestSection, SaveSection, SaveStrategy, WorldSection,
};
use crate::foglet::{ContextSource, FogletContext};
use crate::roles::FogletRole;
use crate::world_db::{WorldDb, WorldDbError};

/// Test-only harness with one shared world DB and per-user roots.
#[derive(Debug)]
pub struct MultiUserHarness {
    tempdir: TempDir,
    world_db_path: PathBuf,
    world_db: WorldDb,
    config: GameConfig,
    users: HashMap<String, HarnessUser>,
}

/// Builder for [`MultiUserHarness`].
#[derive(Debug, Default)]
pub struct MultiUserHarnessBuilder {
    users: Vec<UserSpec>,
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

    /// Build the harness.
    pub fn build(self) -> Result<MultiUserHarness, MultiUserHarnessError> {
        if self.users.is_empty() {
            return Err(MultiUserHarnessError::NoUsers);
        }

        let tempdir =
            tempfile::tempdir().map_err(|source| MultiUserHarnessError::TempDir { source })?;
        let world_db_path = tempdir.path().join("world").join("world.sqlite");
        let world_db = WorldDb::open(&world_db_path)
            .map_err(|source| MultiUserHarnessError::WorldDb { source })?;
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
            users.insert(spec.handle, HarnessUser { foglet, save_root });
        }

        Ok(MultiUserHarness {
            tempdir,
            world_db_path,
            world_db,
            config: harness_config(),
            users,
        })
    }
}

fn harness_config() -> GameConfig {
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
        multiplayer: None,
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
    use crate::roles::FogletRole;

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
}
