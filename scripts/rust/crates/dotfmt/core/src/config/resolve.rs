use std::collections::HashMap;
use std::path::Component;
use std::sync::{Mutex, OnceLock};

use super::*;

type Resolution = Result<Arc<Effective>, Diagnostic>;
type Cached = Arc<OnceLock<Resolution>>;

pub struct Resolver {
    cwd: PathBuf,
    global: Option<PathBuf>,
    overlay: Option<ConfigOverlay>,
    base: OnceLock<Resolution>,
    directories: Mutex<HashMap<PathBuf, Cached>>,
    overlaid: Mutex<HashMap<PathBuf, Cached>>,
    finalized: Mutex<HashMap<PathBuf, Cached>>,
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Resolver {
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join(".config"))
            });
        Self::with_paths(cwd, base.map(|base| base.join("dotfmt").join(CONFIG_NAME)))
    }

    pub fn with_paths(cwd: PathBuf, global_config: Option<PathBuf>) -> Self {
        Self::with_paths_overlay(cwd, global_config, None)
    }

    pub fn with_paths_overlay(
        cwd: PathBuf,
        global_config: Option<PathBuf>,
        overlay: Option<ConfigOverlay>,
    ) -> Self {
        Self {
            cwd,
            global: global_config,
            overlay: overlay.filter(|overlay| !overlay.is_empty()),
            base: OnceLock::new(),
            directories: Mutex::new(HashMap::new()),
            overlaid: Mutex::new(HashMap::new()),
            finalized: Mutex::new(HashMap::new()),
        }
    }

    pub fn for_file(&self, path: &Path) -> Resolution {
        let path = self.absolute_path(path)?;
        self.for_directory(path.parent().unwrap_or(&self.cwd))
    }

    pub fn for_directory(&self, path: &Path) -> Resolution {
        let directory = self.absolute_path(path)?;
        let effective = match &self.overlay {
            Some(overlay) => self.overlaid_directory(&directory, overlay)?,
            None => self.file_directory(&directory)?,
        };
        if effective.has_language_blocks() {
            return Ok(effective);
        }
        let cell = cache_entry(&self.finalized, &directory)?;
        cell.get_or_init(|| {
            let mut effective = (*effective).clone();
            effective.enable_default_languages();
            Ok(Arc::new(effective))
        })
        .clone()
    }

    fn file_directory(&self, directory: &Path) -> Resolution {
        let cell = cache_entry(&self.directories, directory)?;
        cell.get_or_init(|| {
            let inherited = if let Some(parent) = directory.parent() {
                self.file_directory(parent)?
            } else {
                self.base
                    .get_or_init(|| {
                        let mut effective = Effective {
                            cwd: self.cwd.clone(),
                            ..Effective::default()
                        };
                        if let Some(global) = &self.global
                            && let Some(layer) = read_layer(global, &self.cwd)?
                        {
                            effective.apply(layer);
                        }
                        Ok(Arc::new(effective))
                    })
                    .clone()?
            };
            let config = directory.join(CONFIG_NAME);
            if self.global.as_ref() == Some(&config) {
                return Ok(inherited);
            }
            let Some(layer) = read_layer(&config, directory)? else {
                return Ok(inherited);
            };
            let mut effective = (*inherited).clone();
            effective.apply(layer);
            Ok(Arc::new(effective))
        })
        .clone()
    }

    fn overlaid_directory(&self, directory: &Path, overlay: &ConfigOverlay) -> Resolution {
        let cell = cache_entry(&self.overlaid, directory)?;
        cell.get_or_init(|| {
            let mut effective = (*self.file_directory(directory)?).clone();
            effective.apply_overlay(overlay.clone());
            Ok(Arc::new(effective))
        })
        .clone()
    }

    pub fn absolute_path(&self, path: &Path) -> Result<PathBuf, Diagnostic> {
        absolute(&self.cwd, path)
    }
}

fn cache_entry(
    directories: &Mutex<HashMap<PathBuf, Cached>>,
    directory: &Path,
) -> Result<Cached, Diagnostic> {
    Ok(directories
        .lock()
        .map_err(|_| {
            Diagnostic::new(
                DiagnosticKind::Internal,
                "configuration cache lock poisoned",
            )
        })?
        .entry(directory.to_path_buf())
        .or_default()
        .clone())
}

pub(super) fn absolute(cwd: &Path, path: &Path) -> Result<PathBuf, Diagnostic> {
    let supplied = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in supplied.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if std::fs::symlink_metadata(&normalized)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
                {
                    normalized = std::fs::canonicalize(&normalized).map_err(|error| {
                        Diagnostic::new(DiagnosticKind::Io, error.to_string())
                            .with_path(&normalized)
                    })?;
                }
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

fn read_layer(path: &Path, root: &Path) -> Result<Option<Layer>, Diagnostic> {
    match std::fs::read_to_string(path) {
        Ok(source) => parse::parse(&source, path, root).map(|layer| {
            Some(Layer {
                source: Some(path.to_path_buf()),
                ..layer
            })
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Diagnostic::new(DiagnosticKind::Io, error.to_string()).with_path(path)),
    }
}
