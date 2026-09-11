use super::record::Run;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const STORE_SCHEMA: u32 = 1;
pub type Baselines = BTreeMap<String, BTreeMap<String, String>>;

#[derive(Deserialize, Serialize)]
struct Manifest {
    schema: u32,
}

#[derive(Deserialize, Serialize)]
struct HostBaselines {
    schema: u32,
    pins: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct Store {
    pub root: PathBuf,
}

pub struct Lock {
    _file: File,
}

pub fn measurement_lock() -> Result<Lock, String> {
    let path = std::env::var_os("HWTUNE_MEASUREMENT_LOCK")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/hwtune-measurement.lock"));
    let file = machine_lock_file(&path)?;
    file.try_lock_exclusive()
        .map_err(|_| "another benchmark is already running".to_string())?;
    Ok(Lock { _file: file })
}

fn machine_lock_file(path: &Path) -> Result<File, String> {
    use std::os::unix::fs::PermissionsExt;
    let open = || {
        rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map(File::from)
    };
    let file = match open() {
        Ok(file) => file,
        Err(rustix::io::Errno::NOENT) => {
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            let temporary =
                tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o444))
                .map_err(|error| error.to_string())?;
            match temporary.persist_noclobber(path) {
                Ok(created) => drop(created),
                Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(format!("{}: {error}", path.display())),
            }
            open().map_err(|error| format!("{}: {error}", path.display()))?
        }
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err(format!(
            "{}: measurement lock must be a regular file",
            path.display()
        ));
    }
    Ok(file)
}

fn acquire_lock(path: &Path) -> Result<Lock, String> {
    let parent = path.parent().ok_or("missing lock directory")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.try_lock_exclusive()
        .map_err(|_| "another benchmark is already running".to_string())?;
    Ok(Lock { _file: file })
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn discover() -> Self {
        Self::new(
            std::env::var_os("HWTUNE_BENCHMARKS")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| sysinfo::inventory::repo_root().join("benchmarks")),
        )
    }

    fn host_path(&self, host: &str) -> Result<PathBuf, String> {
        component(host)?;
        let hosts = self.root.join("hosts");
        directory(&hosts)?;
        let path = hosts.join(host);
        directory(&path)?;
        Ok(path)
    }

    pub fn run_path(&self, host: &str, id: &str) -> Result<PathBuf, String> {
        self.record_path(host, "runs", id)
    }

    pub fn stability_path(&self, host: &str, id: &str) -> Result<PathBuf, String> {
        self.record_path(host, "stability", id)
    }

    pub fn tuning_path(&self, host: &str, id: &str) -> Result<PathBuf, String> {
        self.record_path(host, "tuning", id)
    }

    fn record_path(&self, host: &str, kind: &str, id: &str) -> Result<PathBuf, String> {
        component(id)?;
        let path = self.host_path(host)?.join(kind);
        directory(&path)?;
        Ok(path.join(format!("{id}.json")))
    }

    fn initialized(&self) -> Result<bool, String> {
        let path = self.root.join("store.json");
        match read_json::<Manifest>(&path)? {
            Some(manifest) => {
                schema(manifest.schema, &path)?;
                Ok(true)
            }
            None => {
                let entries = match fs::read_dir(&self.root) {
                    Ok(entries) => entries,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                    Err(error) => return Err(format!("{}: {error}", self.root.display())),
                };
                for entry in entries {
                    let entry = entry.map_err(|error| error.to_string())?;
                    if entry.file_name() != ".lock" {
                        return Err(format!(
                            "{}: benchmark store is missing store.json",
                            self.root.display()
                        ));
                    }
                }
                Ok(false)
            }
        }
    }

    pub fn initialize(&self) -> Result<(), String> {
        if self.initialized()? {
            return Ok(());
        }
        let path = self.root.join("store.json");
        match atomic_create(
            &path,
            &json_bytes(&Manifest {
                schema: STORE_SCHEMA,
            })?,
        ) {
            Ok(()) => Ok(()),
            Err(error) => {
                if self.initialized()? {
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    }

    pub fn exclusive(&self) -> Result<Lock, String> {
        let lock = acquire_lock(&self.root.join(".lock"))?;
        self.initialize()?;
        Ok(lock)
    }

    pub fn save_run(&self, run: &Run) -> Result<PathBuf, String> {
        let path = self.run_path(&run.host, &run.run_id)?;
        validate_run(run, &path)?;
        self.initialize()?;
        atomic_create(&path, &json_bytes(run)?)?;
        Ok(path)
    }

    pub fn load_run(&self, host: &str, id: &str) -> Result<Option<Run>, String> {
        let path = self.run_path(host, id)?;
        if !self.initialized()? {
            return Ok(None);
        }
        let run = read_json::<Run>(&path)?;
        if let Some(run) = &run {
            validate_run(run, &path)?;
            if run.host != host || run.run_id != id {
                return Err(format!(
                    "{}: run identity does not match its path",
                    path.display()
                ));
            }
        }
        Ok(run)
    }

    pub fn known_hosts(&self) -> Result<Vec<String>, String> {
        if !self.initialized()? {
            return Ok(Vec::new());
        }
        let entries = match fs::read_dir(self.root.join("hosts")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.to_string()),
        };
        let mut hosts = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            if entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                let host = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| "host name is not UTF-8")?;
                component(&host)?;
                hosts.push(host);
            }
        }
        hosts.sort();
        Ok(hosts)
    }

    pub fn list_runs(&self, host: Option<&str>, grades: &[&str]) -> Result<Vec<Run>, String> {
        if let Some(host) = host {
            component(host)?;
        }
        if !self.initialized()? {
            return Ok(Vec::new());
        }
        let hosts = match host {
            Some(host) => vec![host.to_owned()],
            None => self.known_hosts()?,
        };
        let mut found = Vec::new();
        for host in hosts {
            let entries = match fs::read_dir(self.host_path(&host)?.join("runs")) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.to_string()),
            };
            let mut paths = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|error| error.to_string())?;
                if entry.path().extension().is_some_and(|ext| ext == "json") {
                    paths.push(entry.path());
                }
            }
            paths.sort();
            for path in paths {
                let id = path
                    .file_stem()
                    .and_then(|id| id.to_str())
                    .ok_or("run ID is not UTF-8")?;
                let run = self.load_run(&host, id)?.ok_or_else(|| {
                    format!("{}: run disappeared while reading history", path.display())
                })?;
                if grades.is_empty() || grades.contains(&run.grade.as_str()) {
                    found.push(run);
                }
            }
        }
        found.sort_by(|a, b| {
            b.started
                .cmp(&a.started)
                .then_with(|| a.host.cmp(&b.host))
                .then_with(|| b.run_id.cmp(&a.run_id))
        });
        Ok(found)
    }

    fn host_baselines(&self, host: &str) -> Result<BTreeMap<String, String>, String> {
        let path = self.host_path(host)?.join("baselines.json");
        if !self.initialized()? {
            return Ok(BTreeMap::new());
        }
        let Some(record) = read_json::<HostBaselines>(&path)? else {
            return Ok(BTreeMap::new());
        };
        schema(record.schema, &path)?;
        for (epoch, id) in &record.pins {
            component(epoch)?;
            component(id)?;
        }
        Ok(record.pins)
    }

    pub fn load_baselines(&self) -> Result<Baselines, String> {
        let mut found = Baselines::new();
        for host in self.known_hosts()? {
            let pins = self.host_baselines(&host)?;
            if !pins.is_empty() {
                found.insert(host, pins);
            }
        }
        Ok(found)
    }

    fn save_host_baselines(
        &self,
        host: &str,
        pins: BTreeMap<String, String>,
    ) -> Result<(), String> {
        let path = self.host_path(host)?.join("baselines.json");
        self.initialize()?;
        atomic_write(
            &path,
            &json_bytes(&HostBaselines {
                schema: STORE_SCHEMA,
                pins,
            })?,
        )
    }

    pub fn set_baseline(&self, host: &str, epoch: &str, run_id: &str) -> Result<(), String> {
        component(epoch)?;
        let run = self
            .load_run(host, run_id)?
            .ok_or_else(|| format!("cannot pin missing run {host}:{run_id}"))?;
        if run.epoch() != epoch {
            return Err(format!(
                "run {host}:{run_id} does not belong to epoch {epoch}"
            ));
        }
        let mut pins = self.host_baselines(host)?;
        pins.insert(epoch.into(), run_id.into());
        self.save_host_baselines(host, pins)
    }

    pub fn clear_baseline(&self, host: &str, epoch: &str) -> Result<bool, String> {
        component(epoch)?;
        let mut pins = self.host_baselines(host)?;
        let removed = pins.remove(epoch).is_some();
        if removed {
            self.save_host_baselines(host, pins)?;
        }
        Ok(removed)
    }

    pub fn baseline_run(&self, host: &str, epoch: &str) -> Result<Option<Run>, String> {
        let pins = self.host_baselines(host)?;
        let Some(id) = pins.get(epoch) else {
            return Ok(None);
        };
        let run = self
            .load_run(host, id)?
            .ok_or_else(|| format!("baseline {host}@{epoch} references missing run {id}"))?;
        if run.epoch() != epoch {
            return Err(format!(
                "baseline {host}@{epoch} references a run from another epoch"
            ));
        }
        Ok(Some(run))
    }

    pub fn prunable(&self, host: Option<&str>, keep: usize) -> Result<Vec<Run>, String> {
        let pins = self.load_baselines()?;
        let protected = pins
            .iter()
            .flat_map(|(host, pins)| pins.values().map(move |id| (host.clone(), id.clone())))
            .collect::<BTreeSet<_>>();
        let mut groups: BTreeMap<(String, String), Vec<Run>> = BTreeMap::new();
        for run in self.list_runs(host, &[])? {
            groups
                .entry((run.host.clone(), run.epoch()))
                .or_default()
                .push(run);
        }
        let mut dropped = Vec::new();
        for runs in groups.into_values() {
            let len = runs.len();
            for (index, run) in runs.into_iter().enumerate() {
                if index >= keep
                    && index + 1 < len
                    && !protected.contains(&(run.host.clone(), run.run_id.clone()))
                    && !run.context.as_ref().is_some_and(|context| {
                        context.tuning_session.is_some() || !context.stability_sessions.is_empty()
                    })
                {
                    dropped.push(run);
                }
            }
        }
        Ok(dropped)
    }
}

fn schema(version: u32, path: &Path) -> Result<(), String> {
    if version != STORE_SCHEMA {
        Err(format!(
            "{}: unsupported benchmark storage schema {version}",
            path.display()
        ))
    } else {
        Ok(())
    }
}

fn validate_run(run: &Run, path: &Path) -> Result<(), String> {
    component(&run.host)?;
    component(&run.run_id)?;
    if run.schema != 1 {
        return Err(format!(
            "{}: unsupported benchmark run schema {}",
            path.display(),
            run.schema
        ));
    }
    if run
        .metrics
        .iter()
        .any(|metric| metric.samples.iter().any(|sample| !sample.is_finite()))
    {
        return Err(format!(
            "{}: benchmark contains non-finite samples",
            path.display()
        ));
    }
    Ok(())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(format!(
                "{}: benchmark record must be a regular file",
                path.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn json_bytes(value: &impl Serialize) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn component(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains(['/', '\\', '\n', '\r', '\0', '{', '}', '='])
    {
        Err(format!("invalid benchmark identifier: {value:?}"))
    } else {
        Ok(())
    }
}

fn directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(format!(
            "{}: benchmark directory must not be a symlink or file",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

pub fn atomic_create(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = staged_write(path, bytes)?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    sync_parent(path)
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = staged_write(path, bytes)?;
    temporary
        .persist(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    sync_parent(path)
}

fn staged_write(path: &Path, bytes: &[u8]) -> Result<tempfile::NamedTempFile, String> {
    let parent = path.parent().ok_or("missing parent directory")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    temporary
        .write_all(bytes)
        .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    Ok(temporary)
}

fn sync_parent(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("missing parent directory")?;
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|error| error.to_string())
}
