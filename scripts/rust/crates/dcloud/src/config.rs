use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub host: String,
    pub state_dir: PathBuf,
    pub password_file: PathBuf,
    pub identity_file: PathBuf,
    pub secrets_file: Option<PathBuf>,
    pub rclone_secrets_file: Option<PathBuf>,
    pub rclone_config_file: Option<PathBuf>,
    #[serde(skip)]
    pub runtime_digest: Option<String>,
    pub recipients: Vec<String>,
    pub tools: Tools,
    pub uploads: UploadSettings,
    pub hosts: BTreeMap<String, HostConfig>,
    pub destinations: BTreeMap<String, Destination>,
    pub jobs: BTreeMap<String, Job>,
    pub sync: BTreeMap<String, SyncPair>,
    pub searches: BTreeMap<String, Search>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            host: String::new(),
            state_dir: "~/.local/state/dcloud".into(),
            password_file: "~/.config/dcloud/repository.key".into(),
            identity_file: "~/.config/dcloud/identity.txt".into(),
            secrets_file: None,
            rclone_secrets_file: None,
            rclone_config_file: None,
            runtime_digest: None,
            recipients: Vec::new(),
            tools: Tools::default(),
            uploads: UploadSettings::default(),
            hosts: BTreeMap::new(),
            destinations: BTreeMap::new(),
            jobs: BTreeMap::new(),
            sync: BTreeMap::new(),
            searches: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tools {
    pub restic: PathBuf,
    pub rclone: PathBuf,
    pub timeout_seconds: u64,
}
impl Default for Tools {
    fn default() -> Self {
        Self {
            restic: "restic".into(),
            rclone: "rclone".into(),
            timeout_seconds: 21600,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UploadSettings {
    pub spool_limit_bytes: u64,
    pub max_download_bytes: u64,
    pub max_restore_bytes: u64,
    pub compression_level: i32,
    pub compression_threads: u32,
}

impl Default for UploadSettings {
    fn default() -> Self {
        Self {
            spool_limit_bytes: 100 * 1024 * 1024 * 1024,
            max_download_bytes: 100 * 1024 * 1024 * 1024,
            max_restore_bytes: 1024 * 1024 * 1024 * 1024,
            compression_level: 3,
            compression_threads: 2,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HostConfig {
    pub ssh: Option<String>,
    pub config: Option<String>,
    pub binary: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestinationKind {
    #[default]
    Local,
    Sftp,
    Drive,
    Rest,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Destination {
    pub kind: DestinationKind,
    pub location: String,
    pub offsite: bool,
    pub encrypted: bool,
    pub append_only: bool,
    pub maintenance_owner: Option<String>,
    pub maintenance_location: Option<String>,
    pub quota_bytes: Option<u64>,
}
impl Default for Destination {
    fn default() -> Self {
        Self {
            kind: DestinationKind::Local,
            location: String::new(),
            offsite: false,
            encrypted: true,
            append_only: false,
            maintenance_owner: None,
            maintenance_location: None,
            quota_bytes: None,
        }
    }
}

impl Destination {
    pub fn repository(&self, host: &str, job: &str) -> Result<String> {
        identifier(host)?;
        identifier(job)?;
        let location = self.location.trim_end_matches('/');
        Ok(match self.kind {
            DestinationKind::Local => {
                format!("{}/{host}/{job}", expand(Path::new(location))?.display())
            }
            DestinationKind::Sftp => format!("sftp:{location}/{host}/{job}"),
            DestinationKind::Drive => format!("rclone:{location}/{host}/{job}"),
            DestinationKind::Rest => format!("rest:{location}/{host}/{job}"),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Job {
    pub sources: BTreeMap<String, Vec<PathBuf>>,
    pub destinations: Vec<String>,
    pub required: Vec<String>,
    pub min_copies: usize,
    pub require_offsite: bool,
    pub schedule: Schedule,
    pub retention: Retention,
    pub exclude: Vec<String>,
    pub category: String,
    pub labels: Vec<String>,
    pub bandwidth_kib: u32,
    pub read_concurrency: usize,
    pub spool_limit_bytes: u64,
    pub min_free_bytes: u64,
    pub retries: u32,
    pub retry_delay_seconds: u64,
    pub overdue_hours: u64,
    pub pending_max_age_days: u32,
    pub ac_only: bool,
    pub network_probe: Option<String>,
    pub before: Vec<Vec<String>>,
    pub after: Vec<Vec<String>>,
    pub alert: Vec<String>,
    pub cleanup: Option<Cleanup>,
}
impl Default for Job {
    fn default() -> Self {
        Self {
            sources: BTreeMap::new(),
            destinations: Vec::new(),
            required: Vec::new(),
            min_copies: 1,
            require_offsite: false,
            schedule: Schedule::default(),
            retention: Retention::default(),
            exclude: Vec::new(),
            category: String::new(),
            labels: Vec::new(),
            bandwidth_kib: 0,
            read_concurrency: 2,
            spool_limit_bytes: 100 * 1024 * 1024 * 1024,
            min_free_bytes: 1024 * 1024 * 1024,
            retries: 3,
            retry_delay_seconds: 5,
            overdue_hours: 192,
            pending_max_age_days: 30,
            ac_only: false,
            network_probe: None,
            before: Vec::new(),
            after: Vec::new(),
            alert: Vec::new(),
            cleanup: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Schedule {
    pub enabled: bool,
    pub weekdays: Vec<u32>,
    pub hour: u32,
    pub minute: u32,
    pub timezone: String,
    pub catch_up: bool,
}
impl Default for Schedule {
    fn default() -> Self {
        Self {
            enabled: true,
            weekdays: vec![7],
            hour: 3,
            minute: 0,
            timezone: "Europe/Oslo".into(),
            catch_up: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Retention {
    pub last: usize,
    pub weekly: usize,
    pub monthly: usize,
    pub yearly: usize,
    pub auto: bool,
}
impl Default for Retention {
    fn default() -> Self {
        Self {
            last: 3,
            weekly: 8,
            monthly: 12,
            yearly: 0,
            auto: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cleanup {
    pub min_age_days: u32,
    pub quarantine_days: u32,
    pub min_verified_copies: usize,
}
impl Default for Cleanup {
    fn default() -> Self {
        Self {
            min_age_days: 30,
            quarantine_days: 14,
            min_verified_copies: 2,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SyncPair {
    pub owner: String,
    pub left: String,
    pub right: String,
    pub left_vps: bool,
    pub right_vps: bool,
    pub backup_left: String,
    pub backup_right: String,
    pub max_delete_percent: u32,
    pub bandwidth_kib: u32,
    pub exclude: Vec<String>,
    pub schedule: Schedule,
}
impl Default for SyncPair {
    fn default() -> Self {
        Self {
            owner: String::new(),
            left: String::new(),
            right: String::new(),
            left_vps: false,
            right_vps: false,
            backup_left: String::new(),
            backup_right: String::new(),
            max_delete_percent: 10,
            bandwidth_kib: 0,
            exclude: Vec::new(),
            schedule: Schedule::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Search {
    pub host: Option<String>,
    pub job: Option<String>,
    pub category: Option<String>,
    pub label: Option<String>,
    pub text: Option<String>,
}

pub fn identifier(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 96
            && value
                .bytes()
                .all(|v| v.is_ascii_alphanumeric() || v == b'-' || v == b'_'),
        "invalid identifier: {value:?}"
    );
    Ok(())
}

pub fn expand(path: &Path) -> Result<PathBuf> {
    let result = if path == Path::new("~") {
        home()?
    } else if let Ok(rest) = path.strip_prefix("~/") {
        home()?.join(rest)
    } else {
        path.to_path_buf()
    };
    ensure!(
        !result
            .components()
            .any(|c| matches!(c, Component::ParentDir)),
        "parent components are not allowed: {}",
        result.display()
    );
    Ok(result)
}

pub fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}
pub fn default_path() -> Result<PathBuf> {
    Ok(std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or(home()?.join(".config"))
        .join("dcloud/config.toml"))
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let resolved_path = path
            .canonicalize()
            .with_context(|| format!("config not found: {}; run dcloud init", path.display()))?;
        let text = std::fs::read_to_string(&resolved_path)
            .with_context(|| format!("config not found: {}; run dcloud init", path.display()))?;
        let mut config: Self = toml::from_str(&text).context("invalid config")?;
        config.state_dir = expand(&config.state_dir)?;
        config.password_file = expand(&config.password_file)?;
        config.identity_file = expand(&config.identity_file)?;
        for secret in [&mut config.password_file, &mut config.identity_file] {
            if !secret.is_absolute() {
                *secret = resolved_path
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join(&*secret);
            }
        }
        for secret in [
            &mut config.secrets_file,
            &mut config.rclone_secrets_file,
            &mut config.rclone_config_file,
        ]
        .into_iter()
        .flatten()
        {
            let expanded = expand(secret)?;
            *secret = if expanded.is_absolute() {
                expanded
            } else {
                resolved_path
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join(expanded)
            };
        }
        config.validate()?;
        Ok(config)
    }

    pub fn digest(&self) -> Result<String> {
        match &self.runtime_digest {
            Some(digest) => Ok(digest.clone()),
            None => Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(self)?))),
        }
    }

    pub fn backup_digest(&self, name: &str) -> Result<String> {
        let job = self
            .jobs
            .get(name)
            .with_context(|| format!("unknown backup job: {name}"))?;
        let paths: BTreeSet<_> = job
            .sources
            .get(&self.host)
            .context("backup job has no sources for this host")?
            .iter()
            .map(|path| expand(path))
            .collect::<Result<_>>()?;
        let required: BTreeSet<_> = job.required.iter().collect();
        let drive = job.destinations.iter().any(|name| {
            self.destinations
                .get(name)
                .is_some_and(|target| target.kind == DestinationKind::Drive)
        });
        let value = serde_json::json!({
            "format": 1, "host": self.host, "job": name, "sources": paths,
            "exclude": job.exclude, "cleanup": job.cleanup,
            "required": required, "min_copies": job.min_copies, "require_offsite": job.require_offsite,
            "targets": self.transfer_targets(&job.destinations)?,
            "recovery": self.secrets_file.as_ref().unwrap_or(&self.password_file),
            "rclone": drive.then(|| self.rclone_secrets_file.as_ref().or(self.rclone_config_file.as_ref())),
        });
        Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&value)?)))
    }

    pub fn upload_digest(&self, targets: &[String]) -> Result<String> {
        let encrypted = targets.iter().any(|name| {
            self.destinations
                .get(name)
                .is_some_and(|target| target.encrypted)
        });
        let drive = targets.iter().any(|name| {
            self.destinations
                .get(name)
                .is_some_and(|target| target.kind == DestinationKind::Drive)
        });
        let recipients: BTreeSet<_> = self.recipients.iter().filter(|_| encrypted).collect();
        let value = serde_json::json!({
            "format": 1, "host": self.host, "targets": self.transfer_targets(targets)?, "recipients": recipients,
            "recovery": self.secrets_file.as_ref().unwrap_or(&self.password_file),
            "identity": encrypted.then(|| self.secrets_file.as_ref().unwrap_or(&self.identity_file)),
            "rclone": drive.then(|| self.rclone_secrets_file.as_ref().or(self.rclone_config_file.as_ref())),
        });
        Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&value)?)))
    }

    fn transfer_targets(&self, targets: &[String]) -> Result<BTreeMap<String, serde_json::Value>> {
        targets.iter().map(|name| {
            let target = self.destinations.get(name).with_context(|| format!("unknown destination: {name}"))?;
            let location = match target.kind {
                DestinationKind::Local => expand(Path::new(&target.location))?.to_string_lossy().into_owned(),
                _ => target.location.trim_end_matches('/').to_owned(),
            };
            Ok((name.clone(), serde_json::json!({"kind": target.kind, "location": location.trim_end_matches('/'),
                "encrypted": target.encrypted, "offsite": target.offsite, "append_only": target.append_only})))
        }).collect()
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1,
            "unsupported config version: {}",
            self.version
        );
        identifier(&self.host)?;
        ensure!(self.state_dir.is_absolute(), "state_dir must be absolute");
        ensure!(
            self.password_file.is_absolute() && self.identity_file.is_absolute(),
            "key paths must be absolute"
        );
        ensure!(
            self.tools.timeout_seconds > 0,
            "timeout_seconds must be positive"
        );
        ensure!(
            self.uploads.spool_limit_bytes > 0
                && self.uploads.max_download_bytes > 0
                && self.uploads.max_restore_bytes > 0,
            "upload storage and restore limits must be positive"
        );
        ensure!(
            (1..=22).contains(&self.uploads.compression_level),
            "upload compression level must be 1..22"
        );
        ensure!(
            self.uploads.compression_threads <= 64,
            "upload compression threads must be 0..64"
        );
        for (name, host) in &self.hosts {
            identifier(name)?;
            if let Some(ssh) = &host.ssh {
                ensure!(
                    !ssh.starts_with('-')
                        && !ssh.is_empty()
                        && !ssh.chars().any(char::is_whitespace),
                    "invalid SSH alias for {name}"
                );
            }
        }
        for (name, dest) in &self.destinations {
            identifier(name)?;
            ensure!(name != "spool", "spool is reserved for internal staging");
            ensure!(
                !dest.location.is_empty() && !dest.location.contains(['\n', '\r', '\0']),
                "invalid destination {name}"
            );
            if matches!(dest.kind, DestinationKind::Sftp | DestinationKind::Rest) {
                ensure!(dest.encrypted, "{name}: VPS encryption is required");
            }
            if dest.kind == DestinationKind::Sftp {
                let (host, path) = dest
                    .location
                    .split_once(':')
                    .context("SFTP location must be alias:/absolute/path")?;
                ensure!(
                    !host.is_empty()
                        && !host.starts_with('-')
                        && !host.chars().any(char::is_whitespace)
                        && path.starts_with('/')
                        && !path.split('/').any(|p| p == ".."),
                    "invalid SFTP location for {name}"
                );
            }
            if dest.kind == DestinationKind::Drive {
                ensure!(
                    dest.location
                        .split_once(':')
                        .is_some_and(|(remote, path)| !remote.is_empty()
                            && !path.trim_matches('/').is_empty()),
                    "Drive location must be remote:folder"
                );
            }
            if dest.append_only {
                ensure!(
                    dest.kind == DestinationKind::Rest,
                    "append_only requires an enforced REST server"
                );
            }
            if let Some(owner) = &dest.maintenance_owner {
                identifier(owner)?;
            }
        }
        let mut storage_roots = BTreeSet::new();
        for dest in self.destinations.values() {
            let root = if dest.kind == DestinationKind::Local {
                let path = expand(Path::new(&dest.location))?;
                ensure!(path.is_absolute(), "local storage path must be absolute");
                format!("local:{}", resolve_prefix(&path).display())
            } else {
                format!("{:?}:{}", dest.kind, dest.location.trim_end_matches('/'))
            };
            ensure!(
                storage_roots.insert(root),
                "duplicate storage roots cannot count as independent copies"
            );
        }
        for (name, job) in &self.jobs {
            identifier(name)?;
            ensure!(
                name != "_uploads",
                "_uploads is reserved for portable archives"
            );
            ensure!(!job.sources.is_empty(), "{name}: no sources");
            ensure!(!job.destinations.is_empty(), "{name}: no destinations");
            let unique: BTreeSet<_> = job.destinations.iter().collect();
            ensure!(
                unique.len() == job.destinations.len(),
                "{name}: duplicate destinations"
            );
            ensure!(
                job.min_copies > 0 && job.min_copies <= job.destinations.len(),
                "{name}: invalid min_copies"
            );
            ensure!(
                job.retention.last > 0,
                "{name}: retention.last must keep a recovery point"
            );
            ensure!(
                job.read_concurrency > 0 && job.read_concurrency <= 64,
                "{name}: read_concurrency must be 1..64"
            );
            ensure!(
                job.retries <= 20 && job.retry_delay_seconds <= 300,
                "{name}: retry limits exceeded"
            );
            ensure!(
                job.spool_limit_bytes > 0,
                "{name}: spool limit must be positive"
            );
            validate_schedule(&job.schedule)?;
            for dest in &job.destinations {
                ensure!(
                    self.destinations.contains_key(dest),
                    "{name}: unknown destination {dest}"
                );
            }
            for required in &job.required {
                ensure!(
                    job.destinations.contains(required),
                    "{name}: required destination is not selected: {required}"
                );
            }
            ensure!(
                !job.require_offsite
                    || job
                        .destinations
                        .iter()
                        .any(|d| self.destinations[d].offsite),
                "{name}: no offsite destination"
            );
            for (host, paths) in &job.sources {
                identifier(host)?;
                ensure!(!paths.is_empty(), "{name}/{host}: no source paths");
                if host != &self.host {
                    continue;
                }
                let paths = paths
                    .iter()
                    .map(|p| expand(p))
                    .collect::<Result<Vec<_>>>()?;
                for path in &paths {
                    ensure!(
                        path.is_absolute() && path != Path::new("/") && path != &home()?,
                        "{name}: use an absolute source inside a selected directory"
                    );
                    ensure!(
                        !overlap(path, &self.state_dir),
                        "{name}: source overlaps state directory"
                    );
                    ensure!(
                        !overlap(path, &self.password_file)
                            && !overlap(path, &self.identity_file)
                            && !self
                                .rclone_config_file
                                .as_ref()
                                .is_some_and(|credentials| overlap(path, credentials)),
                        "{name}: source contains recovery credentials"
                    );
                    for target in &job.destinations {
                        let d = &self.destinations[target];
                        if d.kind == DestinationKind::Local {
                            ensure!(
                                !overlap(path, &expand(Path::new(&d.location))?),
                                "{name}: source overlaps destination"
                            );
                        }
                    }
                }
                for (i, a) in paths.iter().enumerate() {
                    for b in paths.iter().skip(i + 1) {
                        ensure!(!overlap(a, b), "{name}: overlapping source paths");
                    }
                }
            }
            for command in job.before.iter().chain(&job.after) {
                ensure!(
                    command.first().is_some_and(|v| !v.is_empty()),
                    "{name}: empty hook"
                );
            }
            if let Some(cleanup) = &job.cleanup {
                ensure!(
                    job.exclude.is_empty(),
                    "{name}: source cleanup cannot remove excluded material; use a separate complete source"
                );
                ensure!(
                    cleanup.quarantine_days > 0
                        && cleanup.min_age_days > 0
                        && cleanup.min_verified_copies > 0
                        && cleanup.min_verified_copies <= job.destinations.len(),
                    "{name}: invalid cleanup policy"
                );
            }
        }
        for (name, pair) in &self.sync {
            identifier(name)?;
            identifier(&pair.owner)?;
            validate_schedule(&pair.schedule)?;
            ensure!(
                !pair.left.is_empty() && !pair.right.is_empty() && pair.left != pair.right,
                "{name}: invalid sync roots"
            );
            ensure!(
                !pair.backup_left.is_empty() && !pair.backup_right.is_empty(),
                "{name}: sync backup directories required"
            );
            ensure!(
                pair.max_delete_percent > 0 && pair.max_delete_percent <= 25,
                "{name}: max_delete_percent must be 1..25"
            );
            for job in self.jobs.values().filter(|j| j.cleanup.is_some()) {
                if let Some(paths) = job.sources.get(&self.host) {
                    for path in paths {
                        let path = expand(path)?;
                        for side in [&pair.left, &pair.right] {
                            if !side.contains(':') {
                                ensure!(
                                    !overlap(&path, &expand(Path::new(side))?),
                                    "{name}: sync overlaps automatic source deletion"
                                );
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn overlap(a: &Path, b: &Path) -> bool {
    let a = resolve_prefix(a);
    let b = resolve_prefix(b);
    a.starts_with(&b) || b.starts_with(&a)
}

fn resolve_prefix(path: &Path) -> PathBuf {
    let mut existing = path;
    let mut missing = Vec::new();
    loop {
        if let Ok(mut resolved) = std::fs::canonicalize(existing) {
            for component in missing.iter().rev() {
                resolved.push(component);
            }
            return resolved;
        }
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                missing.push(name.to_os_string());
                existing = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

fn validate_schedule(s: &Schedule) -> Result<()> {
    ensure!(
        s.hour < 24
            && s.minute < 60
            && !s.weekdays.is_empty()
            && s.weekdays.iter().all(|d| (1..=7).contains(d)),
        "invalid schedule; weekdays use Monday=1 through Sunday=7"
    );
    s.timezone
        .parse::<chrono_tz::Tz>()
        .context("invalid schedule timezone")?;
    if s.weekdays.len() > 7 {
        bail!("too many schedule weekdays");
    }
    Ok(())
}
