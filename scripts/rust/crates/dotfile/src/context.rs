use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug)]
pub struct Context {
    pub root: PathBuf,
    pub home: PathBuf,
    pub state: PathBuf,
    pub targets_file: PathBuf,
    pub packages_config: PathBuf,
    pub packages_doc: PathBuf,
    pub overrides_file: PathBuf,
    pub environment_dir: PathBuf,
    pub process_env: BTreeMap<OsString, OsString>,
}

impl Context {
    pub fn discover() -> Result<Self, String> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set".to_string())?;
        let root = match std::env::var_os("DOTFILE_ROOT") {
            Some(path) => PathBuf::from(path),
            None => compiled_root(),
        };
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        Self::new(root, home, config.join("dotfile"))
    }

    pub fn new(root: PathBuf, home: PathBuf, state: PathBuf) -> Result<Self, String> {
        if !root.join("config").is_dir() && !root.join(".git").exists() {
            return Err(format!(
                "dotfiles repository not found at {}",
                root.display()
            ));
        }
        Ok(Self {
            targets_file: root.join("config/targets.dotfile"),
            packages_config: root.join("config/packages.dotfile"),
            packages_doc: root.join("PACKAGES.md"),
            overrides_file: state.join("overrides"),
            environment_dir: root.join("environment"),
            process_env: BTreeMap::new(),
            root,
            home,
            state,
        })
    }

    pub fn env(&self, name: &str) -> Option<OsString> {
        self.process_env
            .get(OsStr::new(name))
            .cloned()
            .or_else(|| std::env::var_os(name))
    }

    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(program);
        command.envs(&self.process_env);
        command
    }

    pub fn inventory(&self) -> sysinfo::inventory::InventoryContext {
        sysinfo::inventory::InventoryContext {
            root: self.root.clone(),
            host: self
                .env("SYSINFO_HOST")
                .map(|value| value.to_string_lossy().trim().to_string())
                .filter(|value| !value.is_empty()),
            config: self.env("SYSINFO_CONFIG").map(PathBuf::from),
            state_file: self.state.join("host"),
        }
    }

    pub fn profile(&self, requested: Option<&str>) -> Result<String, String> {
        if let Some(profile) = requested.filter(|profile| !profile.is_empty()) {
            return self.require_profile(profile);
        }
        let profile_path = self.state.join("profile");
        let saved = match fs::read_to_string(&profile_path) {
            Ok(saved) => saved,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(format!("read {}: {error}", profile_path.display())),
        }
        .trim_end_matches(['\r', '\n'])
        .to_string();
        if saved.is_empty() {
            return Err(format!(
                "no profile selected; pass one (available: {})",
                self.profiles()?.join(", ")
            ));
        }
        self.require_profile(&saved)
    }

    pub fn profiles(&self) -> Result<Vec<String>, String> {
        let mut found = Vec::new();
        collect_profiles(&self.environment_dir, &self.environment_dir, &mut found)?;
        found.sort();
        Ok(found)
    }

    pub fn manifest(&self, profile: &str) -> PathBuf {
        self.environment_dir.join(profile).join("manifest")
    }

    pub fn save_profile(&self, profile: &str, dry_run: bool) -> Result<(), String> {
        if dry_run {
            return Ok(());
        }
        write_atomic(
            &self.state.join("profile"),
            format!("{profile}\n").as_bytes(),
        )
    }

    fn require_profile(&self, profile: &str) -> Result<String, String> {
        crate::config::validate_relative(profile)?;
        let manifest = self.manifest(profile);
        match fs::metadata(&manifest) {
            Ok(metadata) if metadata.is_file() => Ok(profile.to_string()),
            Ok(_) => Err(format!(
                "no manifest for profile '{profile}' (available: {})",
                self.profiles()?.join(", ")
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(format!(
                "no manifest for profile '{profile}' (available: {})",
                self.profiles()?.join(", ")
            )),
            Err(error) => Err(format!("read {}: {error}", manifest.display())),
        }
    }
}

pub fn write_atomic(path: &Path, content: &[u8]) -> Result<(), String> {
    crate::fs::write_generated(path, content).map(|_| ())
}

fn collect_profiles(directory: &Path, base: &Path, found: &mut Vec<String>) -> Result<(), String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("read {}: {error}", directory.display())),
    };
    let mut entries = entries
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("read {}: {error}", directory.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    if entries.iter().any(|entry| entry.file_name() == "manifest")
        && let Ok(relative) = directory.strip_prefix(base)
    {
        found.push(relative.to_string_lossy().replace('\\', "/"));
    }
    for entry in entries {
        let path = entry.path();
        if entry
            .file_type()
            .map_err(|error| format!("read {}: {error}", path.display()))?
            .is_dir()
        {
            collect_profiles(&path, base, found)?;
        }
    }
    Ok(())
}

fn compiled_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")))
        .to_path_buf()
}
