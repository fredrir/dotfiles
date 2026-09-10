use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use chrono::Utc;
use fs2::FileExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::{NamedTempFile, TempDir};
use uuid::Uuid;

use crate::config::{Config, SyncPair, expand, home, identifier, overlap};
use crate::transport::{run_command, validate_remote};

pub fn run(config: &Config, name: &str, initialize: bool, apply: bool) -> Result<Value> {
    identifier(name)?;
    let pair = config
        .sync
        .get(name)
        .with_context(|| format!("unknown sync pair: {name}"))?;
    ensure!(
        pair.owner == config.host,
        "sync {name} is owned by {}; run it on that host",
        pair.owner
    );
    let directory = config.state_dir.join("sync").join(name);
    private_directory(&directory)?;
    let _lock = SyncLock::acquire(&directory.join("dcloud.lock"))?;
    run_locked(config, name, initialize, apply)
}

pub fn run_due(config: &Config, state: &mut crate::state::State) -> Result<Value> {
    let now = Utc::now();
    let mut pairs = Vec::new();
    let mut errors = Vec::new();
    for (name, pair) in &config.sync {
        if pair.owner != config.host || !pair.schedule.enabled {
            continue;
        }
        let outcome = (|| -> Result<Option<Value>> {
            identifier(name)?;
            let directory = config.state_dir.join("sync").join(name);
            private_directory(&directory)?;
            let _lock = SyncLock::acquire(&directory.join("dcloud.lock"))?;
            let key = format!("{}--{name}", config.host);
            let last: Option<chrono::DateTime<Utc>> = state.load_value("sync_occurrence", &key)?;
            let Some(occurrence) = crate::schedule::due(&pair.schedule, last, now)? else {
                return Ok(None);
            };
            let result = run_locked(config, name, false, true)?;
            state.save_value("sync_occurrence", &key, &occurrence)?;
            Ok(Some(result))
        })();
        match outcome {
            Ok(Some(result)) => pairs.push(result),
            Ok(None) => {}
            Err(error) => errors.push(json!({"pair": name, "error": format!("{error:#}")})),
        }
    }
    Ok(json!({"pairs": pairs, "errors": errors}))
}

fn run_locked(config: &Config, name: &str, initialize: bool, apply: bool) -> Result<Value> {
    identifier(name)?;
    let configured = config
        .sync
        .get(name)
        .with_context(|| format!("unknown sync pair: {name}"))?;
    ensure!(
        configured.owner == config.host,
        "sync {name} is owned by {}; run it on that host",
        configured.owner
    );
    let pair = normalized(configured)?;
    let timeout = Duration::from_secs(config.tools.timeout_seconds);
    for (side, vps) in [(&pair.left, pair.left_vps), (&pair.right, pair.right_vps)] {
        validate_backend(config, side, vps, timeout)?;
    }
    let fingerprint = fingerprint(&pair)?;
    let directory = config.state_dir.join("sync").join(name);
    private_directory(&directory)?;
    let state_path = directory.join("pair.json");
    if !initialize {
        let state: Value = serde_json::from_slice(
            &fs::read(&state_path)
                .context("sync is not initialized; run sync --init to preview initialization")?,
        )?;
        ensure!(
            state["fingerprint"].as_str() == Some(&fingerprint),
            "sync paths, owner, or exclusions changed; preview an explicit --init"
        );
    }
    let preview_directory = if apply { None } else { Some(TempDir::new()?) };
    let workdir = preview_directory
        .as_ref()
        .map_or(directory.as_path(), |temp| temp.path());
    if !apply && !initialize {
        copy_working_state(&directory, workdir)?;
    }
    let sentinel = format!(".dcloud-access-{}", &fingerprint[..16]);
    let sentinel_body = format!("dcloud-sync-access {name} {fingerprint}\n");
    if initialize && apply {
        let mut source = NamedTempFile::new()?;
        source.write_all(sentinel_body.as_bytes())?;
        source.as_file().sync_all()?;
        for side in [&pair.left, &pair.right] {
            let target = join(side, &sentinel);
            run_command(
                rclone_command(config)
                    .args([
                        "copyto",
                        "--immutable",
                        "--ignore-existing",
                        "--retries",
                        "1",
                        "--",
                    ])
                    .arg(source.path())
                    .arg(&target)
                    .stdin(Stdio::null()),
                timeout,
                "sync access setup",
            )?;
        }
    }
    if !initialize || apply {
        for side in [&pair.left, &pair.right] {
            let target = join(side, &sentinel);
            let output = run_command(
                rclone_command(config)
                    .args(["cat", "--"])
                    .arg(target)
                    .stdin(Stdio::null()),
                timeout,
                "sync access check",
            )?;
            ensure!(
                output.stdout == sentinel_body.as_bytes(),
                "sync access marker mismatch: {side}"
            );
        }
    }
    let filters = workdir.join("filters.txt");
    let mut body = format!("+ /{sentinel}\n");
    for exclude in &pair.exclude {
        ensure!(
            !exclude.is_empty() && !exclude.contains(['\0', '\r', '\n']),
            "sync exclusion must be a single pattern"
        );
        body.push_str(&format!("- {exclude}\n"));
    }
    atomic_write(&filters, body.as_bytes())?;
    let run_id = format!("{}-{}", Utc::now().format("%Y%m%dT%H%M%SZ"), Uuid::new_v4());
    let backup_left = join(&pair.backup_left, &run_id);
    let backup_right = join(&pair.backup_right, &run_id);
    let arguments = arguments(
        &pair,
        workdir,
        &filters,
        &sentinel,
        &backup_left,
        &backup_right,
        initialize,
        apply,
    );
    let outcome = run_command(
        rclone_command(config).args(&arguments).stdin(Stdio::null()),
        timeout,
        "bidirectional sync",
    );
    let output = match outcome {
        Ok(output) => output,
        Err(error) => {
            if apply {
                atomic_write(
                    &directory.join("last-run.json"),
                    &serde_json::to_vec(
                        &json!({"at": Utc::now(), "success": false, "initialize": initialize, "error": format!("{error:#}")}),
                    )?,
                )?;
            }
            return Err(
                error.context("sync state retained; inspect the error before an explicit --init")
            );
        }
    };
    let result = json!({
        "pair": name, "owner": config.host, "applied": apply, "initialized": initialize,
        "left": pair.left, "right": pair.right, "access_marker": sentinel,
        "access_marker_planned": initialize && !apply,
        "backup_left": backup_left, "backup_right": backup_right,
        "conflicts": "preserve both; initialization keeps newer and backs up the other version",
        "max_delete_percent": pair.max_delete_percent,
        "log": String::from_utf8_lossy(&output.stderr),
        "log_truncated": output.stderr_truncated,
    });
    if apply {
        atomic_write(
            &state_path,
            &serde_json::to_vec(
                &json!({"fingerprint": fingerprint, "owner": pair.owner, "last_success": Utc::now()}),
            )?,
        )?;
        atomic_write(
            &directory.join("last-run.json"),
            &serde_json::to_vec(
                &json!({"at": Utc::now(), "success": true, "initialize": initialize, "result": result}),
            )?,
        )?;
    }
    Ok(result)
}

pub fn status(config: &Config, name: &str) -> Result<Value> {
    identifier(name)?;
    let pair = config.sync.get(name).context("unknown sync pair")?;
    if pair.owner != config.host {
        return Ok(
            json!({"pair": name, "owner": pair.owner, "status": "unknown; check owner host"}),
        );
    }
    let path = config
        .state_dir
        .join("sync")
        .join(name)
        .join("last-run.json");
    let last_run = match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(error) => return Err(error.into()),
    };
    let initialized = config.state_dir.join("sync").join(name).join("pair.json");
    let last_success: Value = match fs::read(initialized) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes)?["last_success"].clone(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(error) => return Err(error.into()),
    };
    let status = if last_run["success"] == false {
        "failed; inspect before retrying"
    } else if last_success.is_null() {
        "initialization required"
    } else {
        "ready"
    };
    Ok(
        json!({"pair": name, "owner": pair.owner, "status": status, "last_success": last_success, "error": last_run["error"], "last_run": last_run}),
    )
}

#[allow(clippy::too_many_arguments)]
fn arguments(
    pair: &SyncPair,
    workdir: &Path,
    filters: &Path,
    sentinel: &str,
    backup_left: &str,
    backup_right: &str,
    initialize: bool,
    apply: bool,
) -> Vec<std::ffi::OsString> {
    let mut arguments = vec![
        "bisync".into(),
        "--workdir".into(),
        workdir.as_os_str().to_owned(),
        "--filters-file".into(),
        filters.as_os_str().to_owned(),
        "--backup-dir1".into(),
        backup_left.into(),
        "--backup-dir2".into(),
        backup_right.into(),
        "--check-filename".into(),
        sentinel.into(),
        "--max-delete".into(),
        pair.max_delete_percent.to_string().into(),
        "--conflict-resolve".into(),
        "none".into(),
        "--conflict-loser".into(),
        "num".into(),
        "--compare".into(),
        "size,modtime,checksum".into(),
        "--check-sync".into(),
        "true".into(),
        "--resilient".into(),
        "--recover".into(),
        "--retries".into(),
        "3".into(),
        "--links".into(),
        "--metadata".into(),
        "--drive-skip-gdocs".into(),
        "--verbose".into(),
    ];
    if !initialize || apply {
        arguments.push("--check-access".into());
    }
    if initialize {
        arguments.extend(["--resync-mode".into(), "newer".into()]);
    }
    if !apply {
        arguments.push("--dry-run".into());
    }
    if pair.bandwidth_kib > 0 {
        arguments.extend([
            "--bwlimit".into(),
            format!("{}k", pair.bandwidth_kib).into(),
        ]);
    }
    arguments.extend([
        "--".into(),
        pair.left.clone().into(),
        pair.right.clone().into(),
    ]);
    arguments
}

fn normalized(pair: &SyncPair) -> Result<SyncPair> {
    let mut pair = pair.clone();
    pair.left = normalize_path(&pair.left)?;
    pair.right = normalize_path(&pair.right)?;
    pair.backup_left = normalize_path(&pair.backup_left)?;
    pair.backup_right = normalize_path(&pair.backup_right)?;
    ensure!(
        (1..=25).contains(&pair.max_delete_percent),
        "sync delete limit must be 1..25 percent"
    );
    ensure!(
        !paths_overlap(&pair.left, &pair.right)?,
        "sync roots overlap"
    );
    for (side, backup) in [
        (&pair.left, &pair.backup_left),
        (&pair.right, &pair.backup_right),
    ] {
        ensure!(
            namespace(side)? == namespace(backup)?,
            "sync backup must use the same remote as its source"
        );
        for root in [&pair.left, &pair.right] {
            ensure!(
                !paths_overlap(root, backup)?,
                "sync backup overlaps a sync root"
            );
        }
    }
    ensure!(
        !paths_overlap(&pair.backup_left, &pair.backup_right)?,
        "sync backup directories overlap"
    );
    Ok(pair)
}

fn normalize_path(value: &str) -> Result<String> {
    ensure!(
        !value.is_empty() && !value.contains(['\0', '\r', '\n']),
        "invalid sync path"
    );
    if value.contains(':') {
        let (remote, path) = validate_remote(value)?;
        ensure!(
            !path.trim_matches('/').is_empty(),
            "sync requires a directory below the remote root"
        );
        ensure!(
            !path.split('/').any(|p| p == "."),
            "sync path is not normalized"
        );
        Ok(format!("{remote}:{}", path.trim_end_matches('/')))
    } else {
        let path = expand(Path::new(value))?;
        ensure!(
            path.is_absolute() && path != Path::new("/") && path != home()?,
            "sync requires an absolute selected directory"
        );
        Ok(path
            .to_str()
            .context("sync path must be UTF-8")?
            .trim_end_matches('/')
            .into())
    }
}

fn namespace(value: &str) -> Result<&str> {
    if value.contains(':') {
        Ok(validate_remote(value)?.0)
    } else {
        Ok("")
    }
}

fn paths_overlap(left: &str, right: &str) -> Result<bool> {
    if namespace(left)? != namespace(right)? {
        return Ok(false);
    }
    if !left.contains(':') {
        return Ok(overlap(Path::new(left), Path::new(right)));
    }
    let (_, left) = validate_remote(left)?;
    let (_, right) = validate_remote(right)?;
    let left = Path::new(left.trim_matches('/'));
    let right = Path::new(right.trim_matches('/'));
    Ok(left.starts_with(right) || right.starts_with(left))
}

fn validate_backend(config: &Config, path: &str, vps: bool, timeout: Duration) -> Result<()> {
    if !path.contains(':') {
        ensure!(!vps, "VPS sync requires a configured rclone crypt remote");
        ensure!(
            fs::metadata(path)?.is_dir(),
            "sync root is not a directory: {path}"
        );
        return Ok(());
    }
    let (remote, _) = validate_remote(path)?;
    let output = run_command(
        rclone_command(config)
            .args(["listremotes", "--json", "--name", remote, "--exact"])
            .stdin(Stdio::null()),
        timeout,
        "sync remote validation",
    )?;
    let remotes: Vec<Value> = serde_json::from_slice(&output.stdout)?;
    ensure!(
        remotes.len() == 1,
        "configured remote not found or ambiguous: {remote}"
    );
    let kind = remotes[0]["type"].as_str().context("remote type missing")?;
    ensure!(
        !vps || kind == "crypt",
        "VPS sync requires a crypt remote: {remote}"
    );
    ensure!(
        matches!(kind, "crypt" | "drive" | "local"),
        "remote {remote} uses backend {kind}; SSH/VPS and wrapper backends must use an encrypted crypt remote"
    );
    Ok(())
}

fn rclone_command(config: &Config) -> Command {
    let mut command = Command::new(&config.tools.rclone);
    command.args([
        "--crypt-no-data-encryption=false",
        "--crypt-pass-bad-blocks=false",
    ]);
    if let Some(path) = &config.rclone_config_file {
        command.env("RCLONE_CONFIG", path);
    }
    command
}

fn fingerprint(pair: &SyncPair) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&json!({
            "owner": pair.owner, "left": pair.left, "right": pair.right, "left_vps": pair.left_vps,
            "right_vps": pair.right_vps, "exclude": pair.exclude, "backup_left": pair.backup_left,
            "backup_right": pair.backup_right, "format": 1,
        }))?)
    ))
}

fn join(root: &str, name: &str) -> String {
    format!("{}/{name}", root.trim_end_matches('/'))
}

fn private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    ensure!(
        !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "sync state cannot be a symlink"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("state path has no parent")?;
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn copy_working_state(source: &Path, destination: &Path) -> Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_name() == "dcloud.lock"
            || entry.file_name().to_string_lossy().ends_with(".lck")
        {
            continue;
        }
        ensure!(entry.file_type()?.is_file(), "unexpected sync state entry");
        fs::copy(entry.path(), destination.join(entry.file_name()))?;
    }
    Ok(())
}

struct SyncLock {
    _file: File,
}

impl SyncLock {
    fn acquire(path: &Path) -> Result<Self> {
        if let Ok(metadata) = fs::symlink_metadata(path) {
            ensure!(metadata.is_file(), "sync lock is not a regular file");
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        file.try_lock_exclusive()
            .context("sync is already running")?;
        Ok(Self { _file: file })
    }
}

impl Drop for SyncLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self._file);
    }
}

#[cfg(test)]
#[path = "../tests/unit/sync_tests.rs"]
mod tests;
