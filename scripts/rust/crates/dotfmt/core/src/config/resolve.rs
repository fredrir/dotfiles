use std::collections::HashMap;
use std::path::Component;
use std::sync::{Mutex, OnceLock};

use super::*;

type Resolution = Result<Arc<Effective>, Diagnostic>;
type Cached = Arc<OnceLock<Resolution>>;

pub struct Resolver {
    cwd: PathBuf,
    global: Option<PathBuf>,
    base: OnceLock<Resolution>,
    directories: Mutex<HashMap<PathBuf, Cached>>,
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

    /// Constructs an isolated resolver without changing the process environment.
    pub fn with_paths(cwd: PathBuf, global_config: Option<PathBuf>) -> Self {
        Self {
            cwd,
            global: global_config,
            base: OnceLock::new(),
            directories: Mutex::new(HashMap::new()),
        }
    }

    pub fn for_file(&self, path: &Path) -> Resolution {
        let path = self.absolute_path(path)?;
        self.for_directory(path.parent().unwrap_or(&self.cwd))
    }

    pub fn for_directory(&self, path: &Path) -> Resolution {
        let directory = self.absolute_path(path)?;
        let cell = self
            .directories
            .lock()
            .map_err(|_| {
                Diagnostic::new(
                    DiagnosticKind::Internal,
                    "configuration cache lock poisoned",
                )
            })?
            .entry(directory.clone())
            .or_default()
            .clone();
        cell.get_or_init(|| {
            let inherited = if let Some(parent) = directory.parent() {
                self.for_directory(parent)?
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
            let Some(layer) = read_layer(&config, &directory)? else {
                return Ok(inherited);
            };
            let mut effective = (*inherited).clone();
            effective.apply(layer);
            Ok(Arc::new(effective))
        })
        .clone()
    }

    /// Resolves relative and parent components using the same path semantics as
    /// configuration discovery, without following ordinary symlink targets.
    pub fn absolute_path(&self, path: &Path) -> Result<PathBuf, Diagnostic> {
        absolute(&self.cwd, path)
    }
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
                // A symlink followed by `..` names the physical target's parent.
                // Only resolve links at this boundary; ordinary link paths keep
                // their lexical spelling and configuration beside the link.
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
        Ok(source) => parse::parse(&source, path, root).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Diagnostic::new(DiagnosticKind::Io, error.to_string()).with_path(path)),
    }
}
