use super::record::Run;
use fs2::FileExt;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub type Baselines = BTreeMap<String, BTreeMap<String, String>>;
#[derive(Clone, Debug)]
pub struct Store {
    pub root: PathBuf,
}
pub struct Lock {
    _file: File,
}
impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    pub fn discover() -> Self {
        Self::new(
            std::env::var_os("SYSINFO_BENCHMARKS")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| crate::inventory::repo_root().join("benchmarks")),
        )
    }
    pub fn run_path(&self, host: &str, id: &str) -> Result<PathBuf, String> {
        component(host)?;
        component(id)?;
        Ok(self.root.join(host).join(format!("{id}.json")))
    }
    pub fn exclusive(&self) -> Result<Lock, String> {
        fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let path = self.root.join(".lock");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        file.try_lock_exclusive()
            .map_err(|_| "another benchmark is already running".to_string())?;
        file.set_len(0).map_err(|e| e.to_string())?;
        writeln!(file, "{}", std::process::id()).map_err(|e| e.to_string())?;
        Ok(Lock { _file: file })
    }
    pub fn save_run(&self, run: &Run) -> Result<PathBuf, String> {
        let path = self.run_path(&run.host, &run.run_id)?;
        let mut bytes = serde_json::to_vec_pretty(run).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        atomic_write(&path, &bytes)?;
        Ok(path)
    }
    pub fn load_run(&self, host: &str, id: &str) -> Option<Run> {
        load_run(&self.run_path(host, id).ok()?)
    }
    pub fn known_hosts(&self) -> Result<Vec<String>, String> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.to_string()),
        };
        let mut hosts = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                hosts.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        hosts.sort();
        Ok(hosts)
    }
    pub fn list_runs(&self, host: Option<&str>, grades: &[&str]) -> Result<Vec<Run>, String> {
        let hosts = if let Some(host) = host {
            component(host)?;
            vec![host.to_owned()]
        } else {
            self.known_hosts()?
        };
        let mut found = Vec::new();
        for host in hosts {
            let entries = match fs::read_dir(self.root.join(&host)) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.to_string()),
            };
            let mut paths = entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                .collect::<Vec<_>>();
            paths.sort();
            for path in paths {
                if let Some(run) = load_run(&path)
                    && run.host == host
                    && component(&run.run_id).is_ok()
                    && path
                        .file_stem()
                        .is_some_and(|stem| stem == run.run_id.as_str())
                    && (grades.is_empty() || grades.contains(&run.grade.as_str()))
                {
                    found.push(run);
                }
            }
        }
        found.sort_by(|a, b| b.started.cmp(&a.started));
        Ok(found)
    }
    pub fn load_baselines(&self) -> Result<Baselines, String> {
        let path = self.root.join("baselines.dotfile");
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Baselines::new()),
            Err(e) => return Err(e.to_string()),
        };
        let mut found = Baselines::new();
        for entry in
            workstation::blocks::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?
        {
            let entries = found.entry(entry.block).or_default();
            if !entry.opens
                && let Some((epoch, run)) = entry.text.split_once('=')
            {
                let epoch = epoch.trim();
                let run = run.trim();
                if !epoch.is_empty() && !run.is_empty() {
                    component(run)?;
                    entries.insert(epoch.into(), run.into());
                }
            }
        }
        Ok(found)
    }
    pub fn save_baselines(&self, baselines: &Baselines) -> Result<PathBuf, String> {
        let path = self.root.join("baselines.dotfile");
        let mut blocks = Vec::new();
        for (host, pins) in baselines {
            component(host)?;
            if pins.is_empty() {
                continue;
            }
            let mut lines = vec![format!("{host} {{")];
            let width = pins.keys().map(String::len).max().unwrap_or(0);
            for (epoch, run) in pins {
                component(epoch)?;
                component(run)?;
                lines.push(format!("  {epoch:width$} = {run}"));
            }
            lines.push("}".into());
            blocks.push(lines.join("\n"));
        }
        if blocks.is_empty() {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        } else {
            atomic_write(&path, format!("{}\n", blocks.join("\n\n")).as_bytes())?;
        }
        Ok(path)
    }
    pub fn set_baseline(&self, host: &str, epoch: &str, run_id: &str) -> Result<(), String> {
        let mut pins = self.load_baselines()?;
        pins.entry(host.into())
            .or_default()
            .insert(epoch.into(), run_id.into());
        self.save_baselines(&pins)?;
        Ok(())
    }
    pub fn clear_baseline(&self, host: &str, epoch: &str) -> Result<bool, String> {
        let mut pins = self.load_baselines()?;
        let removed = pins
            .get_mut(host)
            .is_some_and(|entries| entries.remove(epoch).is_some());
        pins.retain(|_, entries| !entries.is_empty());
        if removed {
            self.save_baselines(&pins)?;
        }
        Ok(removed)
    }
    pub fn baseline_run(&self, host: &str, epoch: &str) -> Result<Option<Run>, String> {
        Ok(self
            .load_baselines()?
            .get(host)
            .and_then(|pins| pins.get(epoch))
            .and_then(|id| self.load_run(host, id)))
    }
    pub fn total_bytes_written(&self, host: Option<&str>) -> Result<u64, String> {
        Ok(self
            .list_runs(host, &[])?
            .iter()
            .map(|run| run.bytes_written)
            .sum())
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
                {
                    dropped.push(run);
                }
            }
        }
        Ok(dropped)
    }
}
pub fn load_run(path: &Path) -> Option<Run> {
    let bytes = fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    if !value.is_object() {
        return None;
    }
    let run: Run = serde_json::from_value(value).ok()?;
    if run
        .metrics
        .iter()
        .any(|m| m.samples.iter().any(|n| !n.is_finite()))
    {
        return None;
    }
    Some(run)
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
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("missing parent directory")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temporary.write_all(bytes).map_err(|e| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    temporary.persist(path).map_err(|e| e.to_string())?;
    File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
