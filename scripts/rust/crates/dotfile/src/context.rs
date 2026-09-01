use std::fs;
use std::path::{Path, PathBuf};

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
        if !root.join("config/targets.dotfile").is_file() {
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
            root,
            home,
            state,
        })
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
    match fs::read(path) {
        Ok(current) if current == content => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("create temporary file in {}: {error}", parent.display()))?;
    use std::io::Write;
    temporary
        .write_all(content)
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    set_output_permissions(path, temporary.as_file())?;
    temporary
        .persist(path)
        .map_err(|error| format!("replace {}: {}", path.display(), error.error))?;
    Ok(())
}

#[cfg(unix)]
fn set_output_permissions(path: &Path, file: &fs::File) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = match fs::metadata(path) {
        Ok(metadata) => metadata.permissions().mode() & 0o7777,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0o644,
        Err(error) => return Err(format!("read permissions for {}: {error}", path.display())),
    };
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|error| format!("set permissions for {}: {error}", path.display()))
}

#[cfg(not(unix))]
fn set_output_permissions(path: &Path, file: &fs::File) -> Result<(), String> {
    match fs::metadata(path) {
        Ok(metadata) => file
            .set_permissions(metadata.permissions())
            .map_err(|error| format!("set permissions for {}: {error}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("read permissions for {}: {error}", path.display())),
    }
    Ok(())
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
