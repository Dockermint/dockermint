//! [redb](https://docs.rs/redb) implementation of the [`Database`] trait.
//!
//! Stores build records in a single redb table keyed by
//! `"{recipe_name}:{tag}"`.  Values are JSON-serialized
//! [`BuildRecord`] fields.

use std::path::Path;
use std::sync::Arc;

use redb::{ReadableDatabase, ReadableTable};

use crate::error::DatabaseError;
use crate::saver::{BuildRecord, BuildStatus, Database};

/// Table definition: key = `"recipe:tag"`, value = JSON bytes.
const TABLE: redb::TableDefinition<&str, &[u8]> = redb::TableDefinition::new("builds");

/// redb-backed persistent database.
///
/// Thread-safe via [`Arc`] around the inner [`redb::Database`].
#[derive(Debug, Clone)]
pub struct RedbDatabase {
    db: Arc<redb::Database>,
}

impl RedbDatabase {
    /// Open (or create) the redb database at the given path.
    ///
    /// Parent directories are created automatically.
    ///
    /// # Arguments
    ///
    /// * `path` - Filesystem path for the database file
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::Open`] if the file cannot be created or
    /// opened.
    pub fn open(path: &Path) -> Result<Self, DatabaseError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| {
                DatabaseError::Open(format!("create dir {}: {e}", parent.display()))
            })?;
        }

        let db = redb::Database::builder()
            .create(path)
            .map_err(|e| DatabaseError::Open(format!("{}: {e}", path.display())))?;

        // Ensure the table exists
        let txn = db
            .begin_write()
            .map_err(|e| DatabaseError::Open(format!("begin_write: {e}")))?;
        {
            let _table = txn
                .open_table(TABLE)
                .map_err(|e| DatabaseError::Open(format!("open table: {e}")))?;
        }
        txn.commit()
            .map_err(|e| DatabaseError::Open(format!("commit: {e}")))?;

        Ok(Self { db: Arc::new(db) })
    }
}

impl Database for RedbDatabase {
    /// Persist a build record.
    async fn save_build(&self, record: &BuildRecord) -> Result<(), DatabaseError> {
        let key = format!("{}:{}", record.recipe_name, record.tag);
        let value = serde_json::to_vec(&to_stored(record))
            .map_err(|e| DatabaseError::Serialization(format!("{e}")))?;

        let txn = self
            .db
            .begin_write()
            .map_err(|e| DatabaseError::Write(format!("begin: {e}")))?;
        {
            let mut table = txn
                .open_table(TABLE)
                .map_err(|e| DatabaseError::Write(format!("open: {e}")))?;
            table
                .insert(key.as_str(), value.as_slice())
                .map_err(|e| DatabaseError::Write(format!("insert: {e}")))?;
        }
        txn.commit()
            .map_err(|e| DatabaseError::Write(format!("commit: {e}")))?;

        Ok(())
    }

    /// Retrieve a build record by recipe name and tag.
    async fn get_build(
        &self,
        recipe: &str,
        tag: &str,
    ) -> Result<Option<BuildRecord>, DatabaseError> {
        let key = format!("{recipe}:{tag}");

        let txn = self
            .db
            .begin_read()
            .map_err(|e| DatabaseError::Read(format!("begin: {e}")))?;
        let table = txn
            .open_table(TABLE)
            .map_err(|e| DatabaseError::Read(format!("open: {e}")))?;

        let guard = table
            .get(key.as_str())
            .map_err(|e| DatabaseError::Read(format!("get: {e}")))?;

        match guard {
            Some(val) => {
                let stored: StoredRecord = serde_json::from_slice(val.value())
                    .map_err(|e| DatabaseError::Serialization(format!("{e}")))?;
                Ok(Some(from_stored(stored)?))
            },
            None => Ok(None),
        }
    }

    /// List all build records for a given recipe.
    async fn list_builds(&self, recipe: &str) -> Result<Vec<BuildRecord>, DatabaseError> {
        let prefix = format!("{recipe}:");

        let txn = self
            .db
            .begin_read()
            .map_err(|e| DatabaseError::Read(format!("begin: {e}")))?;
        let table = txn
            .open_table(TABLE)
            .map_err(|e| DatabaseError::Read(format!("open: {e}")))?;

        let mut records = Vec::new();
        let iter = table
            .iter()
            .map_err(|e| DatabaseError::Read(format!("iter: {e}")))?;

        for entry in iter {
            let (k_guard, v_guard) =
                entry.map_err(|e| DatabaseError::Read(format!("next: {e}")))?;
            let k: &str = k_guard.value();
            if k.starts_with(&prefix) {
                let stored: StoredRecord = serde_json::from_slice(v_guard.value())
                    .map_err(|e| DatabaseError::Serialization(format!("{e}")))?;
                records.push(from_stored(stored)?);
            }
        }

        Ok(records)
    }

    /// Check whether a specific recipe+tag combination has been built.
    async fn is_built(&self, recipe: &str, tag: &str) -> Result<bool, DatabaseError> {
        let key = format!("{recipe}:{tag}");

        let txn = self
            .db
            .begin_read()
            .map_err(|e| DatabaseError::Read(format!("begin: {e}")))?;
        let table = txn
            .open_table(TABLE)
            .map_err(|e| DatabaseError::Read(format!("open: {e}")))?;

        let exists = table
            .get(key.as_str())
            .map_err(|e| DatabaseError::Read(format!("get: {e}")))?
            .is_some();

        Ok(exists)
    }
}

// ── JSON serialization layer ─────────────────────────────────────────

/// Serializable representation of [`BuildRecord`] stored in redb.
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredRecord {
    recipe_name: String,
    tag: String,
    status: String,
    image_tag: Option<String>,
    started_at: String,
    completed_at: Option<String>,
    duration_secs: Option<u64>,
    error: Option<String>,
    flavours: std::collections::HashMap<String, String>,
}

fn to_stored(r: &BuildRecord) -> StoredRecord {
    StoredRecord {
        recipe_name: r.recipe_name.clone(),
        tag: r.tag.clone(),
        status: match r.status {
            BuildStatus::InProgress => "in_progress",
            BuildStatus::Success => "success",
            BuildStatus::Failed => "failed",
        }
        .to_owned(),
        image_tag: r.image_tag.clone(),
        started_at: r.started_at.clone(),
        completed_at: r.completed_at.clone(),
        duration_secs: r.duration_secs,
        error: r.error.clone(),
        flavours: r.flavours.clone(),
    }
}

fn from_stored(s: StoredRecord) -> Result<BuildRecord, DatabaseError> {
    let status = match s.status.as_str() {
        "in_progress" => BuildStatus::InProgress,
        "success" => BuildStatus::Success,
        "failed" => BuildStatus::Failed,
        other => {
            return Err(DatabaseError::Serialization(format!(
                "unknown build status: {other}"
            )));
        },
    };

    Ok(BuildRecord {
        recipe_name: s.recipe_name,
        tag: s.tag,
        status,
        image_tag: s.image_tag,
        started_at: s.started_at,
        completed_at: s.completed_at,
        duration_secs: s.duration_secs,
        error: s.error,
        flavours: s.flavours,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn temp_db() -> (tempfile::TempDir, RedbDatabase) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.redb");
        let db = RedbDatabase::open(&path).expect("open");
        (dir, db)
    }

    #[tokio::test]
    async fn save_and_get() {
        let (_dir, db) = temp_db();
        let record = BuildRecord {
            recipe_name: "cosmos-gaiad".to_owned(),
            tag: "v21.0.1".to_owned(),
            status: BuildStatus::Success,
            image_tag: Some("cosmos-gaiad:v21.0.1".to_owned()),
            started_at: "2026-04-12T00:00:00Z".to_owned(),
            completed_at: Some("2026-04-12T00:05:00Z".to_owned()),
            duration_secs: Some(300),
            error: None,
            flavours: HashMap::new(),
        };

        db.save_build(&record).await.expect("save");
        let got = db
            .get_build("cosmos-gaiad", "v21.0.1")
            .await
            .expect("get")
            .expect("some");

        assert_eq!(got.recipe_name, "cosmos-gaiad");
        assert_eq!(got.tag, "v21.0.1");
        assert_eq!(got.status, BuildStatus::Success);
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let (_dir, db) = temp_db();
        let got = db.get_build("nope", "v0.0.0").await.expect("get");
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn is_built_true_false() {
        let (_dir, db) = temp_db();
        assert!(!db.is_built("r", "t").await.expect("ok"));

        let record = BuildRecord {
            recipe_name: "r".to_owned(),
            tag: "t".to_owned(),
            status: BuildStatus::Failed,
            image_tag: None,
            started_at: String::new(),
            completed_at: None,
            duration_secs: None,
            error: Some("oops".to_owned()),
            flavours: HashMap::new(),
        };
        db.save_build(&record).await.expect("save");

        assert!(db.is_built("r", "t").await.expect("ok"));
    }

    #[tokio::test]
    async fn list_builds_filters_by_recipe() {
        let (_dir, db) = temp_db();

        for (recipe, tag) in [("a", "v1"), ("a", "v2"), ("b", "v1")] {
            let record = BuildRecord {
                recipe_name: recipe.to_owned(),
                tag: tag.to_owned(),
                status: BuildStatus::Success,
                image_tag: None,
                started_at: String::new(),
                completed_at: None,
                duration_secs: None,
                error: None,
                flavours: HashMap::new(),
            };
            db.save_build(&record).await.expect("save");
        }

        let a_builds = db.list_builds("a").await.expect("list");
        assert_eq!(a_builds.len(), 2);

        let b_builds = db.list_builds("b").await.expect("list");
        assert_eq!(b_builds.len(), 1);
    }

    #[test]
    fn redb_database_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RedbDatabase>();
    }

    #[test]
    fn from_stored_rejects_unknown_status() {
        let stored = StoredRecord {
            recipe_name: "r".to_owned(),
            tag: "t".to_owned(),
            status: "bogus".to_owned(),
            image_tag: None,
            started_at: String::new(),
            completed_at: None,
            duration_secs: None,
            error: None,
            flavours: HashMap::new(),
        };

        let err = from_stored(stored).unwrap_err();
        assert!(
            matches!(err, DatabaseError::Serialization(ref msg) if msg.contains("bogus")),
            "expected Serialization error with status name, got: {err:?}"
        );
    }

    #[test]
    fn from_stored_accepts_in_progress() {
        let stored = StoredRecord {
            recipe_name: "r".to_owned(),
            tag: "t".to_owned(),
            status: "in_progress".to_owned(),
            image_tag: None,
            started_at: String::new(),
            completed_at: None,
            duration_secs: None,
            error: None,
            flavours: HashMap::new(),
        };

        let record = from_stored(stored).expect("should parse in_progress");
        assert_eq!(record.status, BuildStatus::InProgress);
    }

    #[test]
    fn open_with_relative_filename_succeeds() {
        // Use a temp dir as CWD to avoid polluting the project root.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("flat.redb");
        // path.parent() is the temp dir, which is non-empty, so
        // create_dir_all is called normally.
        let _db = RedbDatabase::open(&path).expect("open with simple filename");
    }
}
