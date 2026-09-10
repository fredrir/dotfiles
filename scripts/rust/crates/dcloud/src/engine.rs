use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use hostkit::process::{self, CaptureLimits, CapturedOutput};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_OUTPUT: usize = 64 * 1024 * 1024;
const MAX_ERROR: usize = 128 * 1024;

#[derive(Clone, Debug)]
pub struct Restic {
    pub binary: PathBuf,
    pub repository: String,
    pub password_file: PathBuf,
    pub cache_dir: PathBuf,
    pub bandwidth_kib: u32,
    pub read_concurrency: usize,
    pub timeout: Duration,
    pub rclone: PathBuf,
    pub rclone_config: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupReceipt {
    pub snapshot_id: String,
    pub total_files: u64,
    pub total_bytes: u64,
    pub data_added: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub time: DateTime<Utc>,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub tree: String,
    #[serde(default)]
    pub original: Option<String>,
    #[serde(default)]
    pub summary: Option<Value>,
}

impl Snapshot {
    pub fn job(&self) -> Option<&str> {
        self.tags
            .iter()
            .find_map(|tag| tag.strip_prefix("dcloud.job:"))
    }

    pub fn category(&self) -> Option<&str> {
        self.tags
            .iter()
            .find_map(|tag| tag.strip_prefix("dcloud.category:"))
    }

    pub fn labels(&self) -> Vec<String> {
        self.tags
            .iter()
            .filter_map(|tag| tag.strip_prefix("dcloud.label:").map(str::to_owned))
            .collect()
    }

    pub fn pinned(&self) -> bool {
        self.tags.iter().any(|tag| tag == "dcloud.pin")
    }

    pub fn stable_id(&self) -> &str {
        self.original.as_deref().unwrap_or(&self.id)
    }

    pub fn matches_id(&self, id: &str) -> bool {
        self.id.starts_with(id)
            || self
                .original
                .as_ref()
                .is_some_and(|original| original.starts_with(id))
            || self
                .tags
                .iter()
                .filter_map(|tag| tag.strip_prefix("dcloud.alias:"))
                .any(|alias| alias.starts_with(id))
    }
}

impl Restic {
    pub fn init(&self) -> Result<()> {
        let mut inspect = self.command()?;
        inspect.args(["cat", "config"]);
        let output = self.capture(&mut inspect)?;
        if output.status.success() {
            let config: Value = serde_json::from_slice(&output.stdout)
                .context("invalid restic repository config")?;
            ensure!(
                config.get("version").and_then(Value::as_u64) == Some(2),
                "dcloud requires restic repository format 2 for compression"
            );
            return Ok(());
        }
        if output.status.code() != Some(10) {
            return checked(output, "inspect repository").map(|_| ());
        }
        let mut command = self.command()?;
        command.args(["init", "--repository-version", "2"]);
        checked(self.capture(&mut command)?, "initialize repository")?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn backup(
        &self,
        paths: &[PathBuf],
        host: &str,
        job: &str,
        category: &str,
        labels: &[String],
        excludes: &[String],
        dry_run: bool,
    ) -> Result<Option<BackupReceipt>> {
        self.backup_internal(paths, host, job, category, labels, excludes, dry_run, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn backup_run(
        &self,
        paths: &[PathBuf],
        host: &str,
        job: &str,
        category: &str,
        labels: &[String],
        excludes: &[String],
        run_id: &str,
    ) -> Result<Option<BackupReceipt>> {
        validate_tag_value(run_id)?;
        self.backup_internal(
            paths,
            host,
            job,
            category,
            labels,
            excludes,
            false,
            Some(run_id),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn backup_internal(
        &self,
        paths: &[PathBuf],
        host: &str,
        job: &str,
        category: &str,
        labels: &[String],
        excludes: &[String],
        dry_run: bool,
        run_id: Option<&str>,
    ) -> Result<Option<BackupReceipt>> {
        ensure!(!paths.is_empty(), "backup requires at least one source");
        validate_tag_value(host)?;
        validate_tag_value(job)?;
        validate_tag_value(category)?;
        let mut command = self.command()?;
        command.args([
            "backup",
            "--host",
            host,
            "--compression",
            "auto",
            "--group-by",
            "host,paths",
        ]);
        command.arg("--tag").arg(format!("dcloud.job:{job}"));
        command
            .arg("--tag")
            .arg(format!("dcloud.category:{category}"));
        if let Some(run_id) = run_id {
            command.arg("--tag").arg(format!("dcloud.run:{run_id}"));
            command.arg("--tag").arg("dcloud.capture:pending");
        }
        for label in labels {
            validate_tag_value(label)?;
            command.arg("--tag").arg(format!("dcloud.label:{label}"));
        }
        for pattern in excludes {
            ensure!(!pattern.contains('\0'), "invalid exclusion pattern");
            command.arg("--exclude").arg(pattern);
        }
        if dry_run {
            command.arg("--dry-run");
        }
        command.arg("--");
        for path in paths {
            command.arg(
                path.canonicalize()
                    .with_context(|| format!("cannot resolve backup source {}", path.display()))?,
            );
        }
        let output_file = tempfile::tempfile()?;
        let output = process::output_to_file(&mut command, &output_file, MAX_ERROR, self.timeout)
            .context("cannot execute restic backup")?;
        checked(output, "backup")?;
        let mut summary = None;
        let mut reader = BufReader::new(output_file);
        std::io::Seek::rewind(&mut reader)?;
        let mut line = String::new();
        loop {
            line.clear();
            let bytes = reader
                .by_ref()
                .take(MAX_OUTPUT as u64 + 1)
                .read_line(&mut line)?;
            if bytes == 0 {
                break;
            }
            ensure!(bytes <= MAX_OUTPUT, "restic emitted an oversized JSON line");
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(&line).context("invalid restic backup JSON")?;
            if value.get("message_type").and_then(Value::as_str) == Some("summary") {
                summary = Some(value);
            } else if value.get("message_type").and_then(Value::as_str) == Some("error") {
                bail!("restic reported an incomplete backup");
            }
        }
        let summary = summary.context("restic backup succeeded without a summary")?;
        if dry_run {
            return Ok(None);
        }
        let mut snapshot_id = summary
            .get("snapshot_id")
            .and_then(Value::as_str)
            .context("restic backup did not create a snapshot")?
            .to_owned();
        validate_snapshot_id(&snapshot_id)?;
        if run_id.is_some() {
            snapshot_id = self.set_tags(
                &snapshot_id,
                &["dcloud.capture:complete".to_owned()],
                &["dcloud.capture:pending".to_owned()],
            )?;
        }
        Ok(Some(BackupReceipt {
            snapshot_id,
            total_files: summary
                .get("total_files_processed")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            total_bytes: summary
                .get("total_bytes_processed")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            data_added: summary
                .get("data_added")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        }))
    }

    pub fn snapshots(&self, host: Option<&str>, job: Option<&str>) -> Result<Vec<Snapshot>> {
        let mut command = self.command()?;
        command.arg("snapshots");
        if let Some(host) = host {
            command.arg("--host").arg(host);
        }
        if let Some(job) = job {
            validate_tag_value(job)?;
            command.arg("--tag").arg(format!("dcloud.job:{job}"));
        }
        let output = checked(self.capture(&mut command)?, "list snapshots")?;
        let mut snapshots: Vec<Snapshot> =
            serde_json::from_slice(&output).context("invalid restic snapshots JSON")?;
        snapshots.sort_by(|left, right| {
            left.time
                .cmp(&right.time)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(snapshots)
    }

    pub fn copy_from(&self, source: &Restic, snapshot: &str) -> Result<String> {
        validate_snapshot_id(snapshot)?;
        let source_snapshot = source.snapshot(snapshot)?;
        ensure!(
            !source_snapshot
                .tags
                .iter()
                .any(|tag| tag == "dcloud.capture:pending")
                && (!source_snapshot
                    .tags
                    .iter()
                    .any(|tag| tag.starts_with("dcloud.run:"))
                    || source_snapshot
                        .tags
                        .iter()
                        .any(|tag| tag == "dcloud.capture:complete")),
            "cannot replicate an incomplete dcloud capture"
        );
        let mut command = self.command()?;
        command
            .arg("copy")
            .arg("--from-repo")
            .arg(&source.repository)
            .arg("--from-password-file")
            .arg(&source.password_file)
            .arg(&source_snapshot.id);
        checked(self.capture(&mut command)?, "copy snapshot")?;
        self.snapshots(Some(&source_snapshot.hostname), source_snapshot.job())?
            .into_iter()
            .find(|candidate| {
                (candidate.id == source_snapshot.id
                    || candidate.original.as_deref() == Some(source_snapshot.stable_id()))
                    && candidate.tree == source_snapshot.tree
                    && candidate.paths == source_snapshot.paths
                    && candidate.time == source_snapshot.time
            })
            .map(|snapshot| snapshot.id)
            .context("copied snapshot could not be verified in the destination")
    }

    pub fn snapshot(&self, id: &str) -> Result<Snapshot> {
        validate_snapshot_id(id)?;
        let matches: Vec<_> = self
            .snapshots(None, None)?
            .into_iter()
            .filter(|snapshot| snapshot.matches_id(id))
            .collect();
        ensure!(matches.len() == 1, "snapshot ID is missing or ambiguous");
        Ok(matches.into_iter().next().unwrap())
    }

    pub fn restore(&self, snapshot: &str, destination: &Path, selection: &[PathBuf]) -> Result<()> {
        validate_snapshot_id(snapshot)?;
        let snapshot = self.snapshot(snapshot)?.id;
        ensure!(
            fs::symlink_metadata(destination).is_err(),
            "restore destination already exists"
        );
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = parent
            .canonicalize()
            .context("restore destination parent must exist")?;
        let staging = tempfile::Builder::new()
            .prefix(".dcloud-restic-restore-")
            .tempdir_in(&parent)?;
        let mut command = self.command()?;
        command
            .arg("restore")
            .arg(&snapshot)
            .arg("--target")
            .arg(staging.path())
            .arg("--verify");
        for path in selection {
            ensure!(!path.as_os_str().is_empty(), "empty restore selection");
            ensure!(
                !path
                    .components()
                    .any(|part| matches!(part, Component::ParentDir | Component::Prefix(_))),
                "unsafe restore selection"
            );
            let path = path.to_str().context("restore selection must be UTF-8")?;
            command.arg("--include").arg(escape_glob(path));
        }
        checked(self.capture(&mut command)?, "restore and verify snapshot")?;
        ensure!(
            fs::symlink_metadata(destination).is_err(),
            "restore destination appeared during restore"
        );
        fs::rename(staging.path(), destination).context("cannot publish verified restore")?;
        #[cfg(unix)]
        fs::File::open(&parent)?.sync_all()?;
        Ok(())
    }

    pub fn check(&self, read_data: bool, subset: Option<&str>) -> Result<()> {
        ensure!(
            !(read_data && subset.is_some()),
            "choose full data verification or a subset"
        );
        let mut command = self.command()?;
        command.arg("check");
        if read_data {
            command.arg("--read-data");
        }
        if let Some(subset) = subset {
            ensure!(
                !subset.is_empty() && !subset.chars().any(char::is_control),
                "invalid verification subset"
            );
            command.arg("--read-data-subset").arg(subset);
        }
        checked(self.capture(&mut command)?, "verify repository")?;
        Ok(())
    }

    pub fn ls(&self, snapshot: &str) -> Result<Vec<Value>> {
        validate_snapshot_id(snapshot)?;
        let snapshot = self.snapshot(snapshot)?.id;
        let mut command = self.command()?;
        command.args(["ls", &snapshot]);
        parse_json_lines(&checked(self.capture(&mut command)?, "browse snapshot")?)
    }

    pub fn diff(&self, old: &str, new: &str) -> Result<Value> {
        validate_snapshot_id(old)?;
        validate_snapshot_id(new)?;
        let old = self.snapshot(old)?.id;
        let new = self.snapshot(new)?.id;
        let mut command = self.command()?;
        command.args(["diff", &old, &new]);
        Ok(Value::Array(parse_json_lines(&checked(
            self.capture(&mut command)?,
            "compare snapshots",
        )?)?))
    }

    pub fn stats(&self, snapshot: Option<&str>) -> Result<Value> {
        let mut command = self.command()?;
        command.args(["stats", "--mode", "raw-data"]);
        if let Some(snapshot) = snapshot {
            validate_snapshot_id(snapshot)?;
            command.arg(self.snapshot(snapshot)?.id);
        }
        serde_json::from_slice(&checked(
            self.capture(&mut command)?,
            "inspect repository size",
        )?)
        .context("invalid restic stats JSON")
    }

    pub fn set_tags(&self, snapshot: &str, add: &[String], remove: &[String]) -> Result<String> {
        validate_snapshot_id(snapshot)?;
        ensure!(
            !add.is_empty() || !remove.is_empty(),
            "no tag changes requested"
        );
        let snapshot = self.snapshot(snapshot)?;
        let mut command = self.command()?;
        command.arg("tag");
        command
            .arg("--add")
            .arg(format!("dcloud.alias:{}", snapshot.id));
        for tag in add {
            validate_tag_value(tag)?;
            command.arg("--add").arg(tag);
        }
        for tag in remove {
            validate_tag_value(tag)?;
            ensure!(
                !tag.starts_with("dcloud.alias:"),
                "snapshot identity aliases cannot be removed"
            );
            command.arg("--remove").arg(tag);
        }
        command.arg(&snapshot.id);
        checked(self.capture(&mut command)?, "update snapshot labels")?;
        Ok(self.snapshot(&snapshot.id)?.id)
    }

    pub fn pin(&self, snapshot: &str, pinned: bool) -> Result<String> {
        let tag = vec!["dcloud.pin".to_owned()];
        if pinned {
            self.set_tags(snapshot, &tag, &[])
        } else {
            self.set_tags(snapshot, &[], &tag)
        }
    }

    pub fn forget(&self, ids: &[String], prune: bool) -> Result<()> {
        ensure!(
            !ids.is_empty(),
            "refusing retention without explicit snapshot IDs"
        );
        for id in ids {
            validate_snapshot_id(id)?;
        }
        let snapshots = self.snapshots(None, None)?;
        let mut actual_ids = Vec::new();
        for id in ids {
            let matches: Vec<_> = snapshots
                .iter()
                .filter(|snapshot| snapshot.matches_id(id))
                .collect();
            ensure!(
                matches.len() == 1,
                "retention snapshot ID is missing or ambiguous"
            );
            ensure!(!matches[0].pinned(), "cannot delete a pinned snapshot");
            actual_ids.push(matches[0].id.clone());
        }
        let mut command = self.command()?;
        command.env("RCLONE_DRIVE_USE_TRASH", "false");
        command.arg("forget").args(actual_ids);
        checked(self.capture(&mut command)?, "remove expired snapshots")?;
        if prune {
            let mut command = self.command()?;
            command.env("RCLONE_DRIVE_USE_TRASH", "false");
            command.arg("prune");
            checked(self.capture(&mut command)?, "reclaim expired backup data")?;
        }
        Ok(())
    }

    fn command(&self) -> Result<Command> {
        ensure!(
            !self.repository.is_empty() && !self.repository.contains('\0'),
            "invalid repository location"
        );
        ensure!(
            self.read_concurrency > 0 && self.read_concurrency <= 128,
            "read concurrency must be between 1 and 128"
        );
        ensure!(!self.timeout.is_zero(), "restic timeout must be positive");
        let metadata = fs::symlink_metadata(&self.password_file)
            .context("repository password file is missing")?;
        ensure!(
            metadata.is_file(),
            "repository password must be a regular file"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                metadata.permissions().mode() & 0o077 == 0,
                "repository password file must be private (chmod 600)"
            );
        }
        fs::create_dir_all(&self.cache_dir)?;
        let mut command = Command::new(&self.binary);
        command
            .arg("--repo")
            .arg(&self.repository)
            .arg("--password-file")
            .arg(&self.password_file)
            .arg("--cache-dir")
            .arg(&self.cache_dir)
            .arg("--json")
            .arg("--option")
            .arg(format!(
                "rclone.program={}",
                self.rclone
                    .to_str()
                    .context("rclone executable path must be UTF-8")?
            ))
            .arg("--retry-lock")
            .arg("30s")
            .env("RESTIC_READ_CONCURRENCY", self.read_concurrency.to_string())
            .env("RESTIC_PROGRESS_FPS", "0.1")
            .env_remove("RESTIC_PASSWORD")
            .env_remove("RESTIC_PASSWORD_COMMAND")
            .env_remove("RESTIC_REPOSITORY_FILE")
            .env_remove("RESTIC_HOST")
            .stdin(Stdio::null());
        if self.bandwidth_kib > 0 {
            command
                .arg("--limit-upload")
                .arg(self.bandwidth_kib.to_string())
                .arg("--limit-download")
                .arg(self.bandwidth_kib.to_string());
        }
        if let Some(config) = &self.rclone_config {
            command.env("RCLONE_CONFIG", config);
        }
        Ok(command)
    }

    fn capture(&self, command: &mut Command) -> Result<CapturedOutput> {
        process::output(
            command,
            CaptureLimits {
                stdout: MAX_OUTPUT,
                stderr: MAX_ERROR,
            },
            self.timeout,
        )
        .context("cannot execute restic; install restic and ensure it is on PATH")
    }
}

fn checked(output: CapturedOutput, operation: &str) -> Result<Vec<u8>> {
    if !output.status.success() {
        if output.status.code() == Some(3) {
            bail!(
                "restic {operation} is incomplete; unreadable source data or failed removals: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        bail!(
            "restic {operation} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    ensure!(
        !output.stdout_truncated,
        "restic {operation} output exceeded the capture limit"
    );
    Ok(output.stdout)
}

fn parse_json_lines(bytes: &[u8]) -> Result<Vec<Value>> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
        .map(|line| serde_json::from_slice(line).context("invalid restic JSON output"))
        .collect()
}

fn validate_snapshot_id(id: &str) -> Result<()> {
    ensure!(
        (8..=64).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "snapshot ID must contain 8 to 64 hexadecimal characters"
    );
    Ok(())
}

fn validate_tag_value(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && !value.contains(',') && !value.chars().any(char::is_control),
        "host, job, category and labels must be nonempty and cannot contain commas or control characters"
    );
    Ok(())
}

fn escape_glob(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for character in path.chars() {
        if matches!(character, '*' | '?' | '[' | ']' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

#[cfg(test)]
#[path = "../tests/unit/engine_tests.rs"]
mod tests;
