use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Planned,
    BackingUp,
    Replicating,
    Committed,
    Degraded,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplicaState {
    Pending,
    Uploaded,
    Verified,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplicaReceipt {
    pub destination: String,
    pub snapshot: Option<String>,
    pub offsite: bool,
    pub state: ReplicaState,
    pub verified_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub full_verified_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub host: String,
    pub job: String,
    pub started: DateTime<Utc>,
    pub state: RunState,
    pub config_hash: String,
    pub snapshot: Option<String>,
    pub replicas: BTreeMap<String, ReplicaReceipt>,
    pub error: Option<String>,
    pub last_restore: Option<DateTime<Utc>>,
}

impl RunRecord {
    pub fn new(host: &str, job: &str, config_hash: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            host: host.to_owned(),
            job: job.to_owned(),
            started: Utc::now(),
            state: RunState::Planned,
            config_hash: config_hash.to_owned(),
            snapshot: None,
            replicas: BTreeMap::new(),
            error: None,
            last_restore: None,
        }
    }
}

pub struct JobLock {
    file: File,
}

impl Drop for JobLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

pub struct State {
    connection: Connection,
    root: PathBuf,
}

impl State {
    pub fn open_readonly(root: &Path) -> Result<Option<Self>> {
        let path = root.join("state.sqlite3");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("inspect dcloud state database"),
        };
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "state database must be a regular file"
        );
        let path = root
            .canonicalize()
            .context("resolve state directory")?
            .join("state.sqlite3");
        let connection = Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .context("read dcloud state database")?;
        connection.busy_timeout(Duration::from_secs(1))?;
        Ok(Some(Self {
            connection,
            root: root.to_owned(),
        }))
    }

    pub fn open(root: &Path) -> Result<Self> {
        private_directory(root)?;
        let path = root.join("state.sqlite3");
        ensure!(
            !fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()),
            "state database must not be a symbolic link"
        );
        let connection = Connection::open(&path).context("open dcloud state database")?;
        private_file(&path)?;
        connection.busy_timeout(Duration::from_secs(30))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS runs (
                 id TEXT PRIMARY KEY,
                 host TEXT NOT NULL,
                 job TEXT NOT NULL,
                 started TEXT NOT NULL,
                 state TEXT NOT NULL,
                 record TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS runs_by_job ON runs(host, job, started);
             CREATE TABLE IF NOT EXISTS occurrences (
                 host TEXT NOT NULL,
                 job TEXT NOT NULL,
                 scheduled_at TEXT NOT NULL,
                 run_id TEXT NOT NULL REFERENCES runs(id),
                 PRIMARY KEY(host, job)
             );
             CREATE TABLE IF NOT EXISTS catalog (
                 destination TEXT NOT NULL,
                 snapshot TEXT NOT NULL,
                 fetched_at TEXT NOT NULL,
                 manifest TEXT NOT NULL,
                 PRIMARY KEY(destination, snapshot)
             );
             CREATE TABLE IF NOT EXISTS values_store (
                 namespace TEXT NOT NULL,
                 name TEXT NOT NULL,
                 value TEXT NOT NULL,
                 PRIMARY KEY(namespace, name)
             );",
        )?;
        Ok(Self {
            connection,
            root: root.to_owned(),
        })
    }

    pub fn lock(&self, name: &str) -> Result<JobLock> {
        let directory = self.root.join("locks");
        private_directory(&directory)?;
        let digest = format!("{:x}", Sha256::digest(name.as_bytes()));
        let path = directory.join(format!("{digest}.lock"));
        ensure!(
            !fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()),
            "job lock must not be a symbolic link"
        );
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(&path).context("open job lock")?;
        fs2::FileExt::try_lock_exclusive(&file)
            .with_context(|| format!("another dcloud process owns {name}"))?;
        Ok(JobLock { file })
    }

    pub fn save_run(&mut self, run: &RunRecord) -> Result<()> {
        ensure!(
            !run.id.is_empty() && !run.host.is_empty() && !run.job.is_empty(),
            "run ID, host and job are required"
        );
        if matches!(run.state, RunState::Committed | RunState::Degraded) {
            ensure!(run.snapshot.is_some(), "a committed run needs a snapshot");
        }
        for (destination, receipt) in &run.replicas {
            if receipt.state == ReplicaState::Verified {
                ensure!(
                    receipt.verified_at.is_some()
                        && receipt.snapshot.is_some()
                        && receipt.destination == *destination,
                    "verified replica {destination} needs its destination, timestamp and snapshot"
                );
            }
        }
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let previous: Option<String> = transaction
            .query_row("SELECT record FROM runs WHERE id = ?1", [&run.id], |row| {
                row.get(0)
            })
            .optional()?;
        if let Some(previous) = previous {
            let previous: RunRecord = serde_json::from_str(&previous)?;
            ensure!(
                previous.host == run.host
                    && previous.job == run.job
                    && previous.started == run.started
                    && previous.config_hash == run.config_hash,
                "existing run identity cannot change"
            );
            ensure!(
                !matches!(previous.state, RunState::Committed | RunState::Degraded)
                    || matches!(run.state, RunState::Committed | RunState::Degraded),
                "a committed run cannot become incomplete"
            );
            ensure!(
                !matches!(previous.state, RunState::Committed | RunState::Degraded)
                    || previous.snapshot == run.snapshot,
                "a committed run cannot change snapshots"
            );
        }
        transaction.execute(
            "INSERT INTO runs(id, host, job, started, state, record) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET state=excluded.state, record=excluded.record",
            params![run.id, run.host, run.job, run.started.to_rfc3339(), serde_json::to_string(&run.state)?, serde_json::to_string(run)?],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn load_run(&self, id: &str) -> Result<Option<RunRecord>> {
        let value: Option<String> = self
            .connection
            .query_row("SELECT record FROM runs WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        value
            .map(|value| serde_json::from_str(&value).map_err(Into::into))
            .transpose()
    }

    pub fn runs(&self) -> Result<Vec<RunRecord>> {
        let mut statement = self
            .connection
            .prepare("SELECT record FROM runs ORDER BY started DESC, id")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn set_occurrence(
        &mut self,
        host: &str,
        job: &str,
        scheduled_at: DateTime<Utc>,
        run_id: &str,
    ) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let serialized: String =
            transaction.query_row("SELECT record FROM runs WHERE id = ?1", [run_id], |row| {
                row.get(0)
            })?;
        let run: RunRecord = serde_json::from_str(&serialized)?;
        ensure!(
            run.host == host
                && run.job == job
                && matches!(run.state, RunState::Committed | RunState::Degraded),
            "only a committed run for the same host and job completes an occurrence"
        );
        let previous: Option<String> = transaction
            .query_row(
                "SELECT scheduled_at FROM occurrences WHERE host = ?1 AND job = ?2",
                params![host, job],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(previous) = previous {
            let previous = DateTime::parse_from_rfc3339(&previous)?.with_timezone(&Utc);
            ensure!(
                scheduled_at >= previous,
                "completed occurrence cannot move backwards"
            );
        }
        transaction.execute(
            "INSERT INTO occurrences(host, job, scheduled_at, run_id) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(host, job) DO UPDATE SET scheduled_at=excluded.scheduled_at, run_id=excluded.run_id",
            params![host, job, scheduled_at.to_rfc3339(), run_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn occurrence(&self, host: &str, job: &str) -> Result<Option<DateTime<Utc>>> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT scheduled_at FROM occurrences WHERE host = ?1 AND job = ?2",
                params![host, job],
                |row| row.get(0),
            )
            .optional()?;
        value
            .map(|value| Ok(DateTime::parse_from_rfc3339(&value)?.with_timezone(&Utc)))
            .transpose()
    }

    pub fn cache_manifest<T: Serialize>(
        &self,
        destination: &str,
        snapshot: &str,
        manifest: &T,
    ) -> Result<()> {
        self.connection.execute(
            "INSERT INTO catalog(destination, snapshot, fetched_at, manifest) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(destination, snapshot) DO UPDATE SET fetched_at=excluded.fetched_at, manifest=excluded.manifest",
            params![destination, snapshot, Utc::now().to_rfc3339(), serde_json::to_string(manifest)?],
        )?;
        Ok(())
    }

    pub fn cached_manifests<T: serde::de::DeserializeOwned>(
        &self,
        destination: &str,
    ) -> Result<Vec<(String, DateTime<Utc>, T)>> {
        let mut statement = self.connection.prepare("SELECT snapshot, fetched_at, manifest FROM catalog WHERE destination = ?1 ORDER BY snapshot")?;
        let rows = statement.query_map([destination], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (snapshot, fetched_at, manifest) = row?;
            Ok((
                snapshot,
                DateTime::parse_from_rfc3339(&fetched_at)?.with_timezone(&Utc),
                serde_json::from_str(&manifest)?,
            ))
        })
        .collect()
    }

    pub fn save_value<T: Serialize>(&self, namespace: &str, name: &str, value: &T) -> Result<()> {
        self.connection.execute(
            "INSERT INTO values_store(namespace, name, value) VALUES (?1, ?2, ?3)
             ON CONFLICT(namespace, name) DO UPDATE SET value=excluded.value",
            params![namespace, name, serde_json::to_string(value)?],
        )?;
        Ok(())
    }

    pub fn load_value<T: serde::de::DeserializeOwned>(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Option<T>> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM values_store WHERE namespace = ?1 AND name = ?2",
                params![namespace, name],
                |row| row.get(0),
            )
            .optional()?;
        value
            .map(|value| serde_json::from_str(&value).map_err(Into::into))
            .transpose()
    }

    pub fn values<T: serde::de::DeserializeOwned>(
        &self,
        namespace: &str,
    ) -> Result<Vec<(String, T)>> {
        let mut statement = self
            .connection
            .prepare("SELECT name, value FROM values_store WHERE namespace = ?1 ORDER BY name")?;
        let rows = statement.query_map([namespace], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (name, value) = row?;
            Ok((name, serde_json::from_str(&value)?))
        })
        .collect()
    }

    pub fn list_values<T: serde::de::DeserializeOwned>(
        &self,
        namespace: &str,
    ) -> Result<Vec<(String, T)>> {
        self.values(namespace)
    }

    pub fn replace_cache<T: Serialize>(
        &mut self,
        destination: &str,
        snapshots: &[(String, T)],
    ) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        transaction.execute("DELETE FROM catalog WHERE destination = ?1", [destination])?;
        let now = Utc::now().to_rfc3339();
        for (snapshot, manifest) in snapshots {
            transaction.execute("INSERT INTO catalog(destination, snapshot, fetched_at, manifest) VALUES (?1, ?2, ?3, ?4)",
                params![destination, snapshot, now, serde_json::to_string(manifest)?])?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn clear_catalog(&self, destination: &str) -> Result<()> {
        self.connection
            .execute("DELETE FROM catalog WHERE destination = ?1", [destination])?;
        Ok(())
    }
}

fn private_directory(path: &Path) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "state directory must be a real directory"
        );
    } else {
        fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn private_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    if !path.is_file() {
        bail!("state database is not a regular file");
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/state_tests.rs"]
mod tests;
