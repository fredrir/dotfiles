use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Resolver {
    pub root: PathBuf,
    pub home: PathBuf,
    pub current_exe: Option<PathBuf>,
    pub manifest: Option<PathBuf>,
}

impl Resolver {
    pub fn discover(root: PathBuf) -> Self {
        Self {
            root,
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default(),
            current_exe: std::env::current_exe().ok(),
            manifest: std::env::var_os("DOTFILE_DEV_BUILD_MANIFEST").map(PathBuf::from),
        }
    }

    pub fn resolve(&self, program: &str) -> Result<Option<PathBuf>, String> {
        Ok(self.resolve_many(&[program.into()])?.remove(program))
    }

    pub fn resolve_many(&self, programs: &[String]) -> Result<BTreeMap<String, PathBuf>, String> {
        for program in programs {
            if program.is_empty()
                || !program
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
            {
                return Err(format!("invalid native command name: {program:?}"));
            }
        }
        let mut found = BTreeMap::new();
        if let Some(manifest) = &self.manifest {
            let text = fs::read_to_string(manifest)
                .map_err(|e| format!("read {}: {e}", manifest.display()))?;
            let mut artifacts = BTreeMap::new();
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                let artifact: serde_json::Value = serde_json::from_str(line)
                    .map_err(|e| format!("{}: {e}", manifest.display()))?;
                if artifact["reason"] == "compiler-artifact"
                    && artifact["profile"]["test"] != true
                    && let (Some(name), Some(path)) = (
                        artifact["target"]["name"].as_str(),
                        artifact["executable"].as_str(),
                    )
                {
                    artifacts.insert(name.to_string(), PathBuf::from(path));
                }
            }
            for program in programs {
                if let Some(path) = artifacts.get(binary_name(program))
                    && is_executable(path)?
                {
                    found.insert(program.clone(), path.clone());
                }
            }
            return Ok(found);
        }
        for program in programs {
            for path in self.candidates(binary_name(program)) {
                if is_executable(&path)? {
                    found.insert(program.clone(), path);
                    break;
                }
            }
        }
        Ok(found)
    }

    fn candidates(&self, name: &str) -> Vec<PathBuf> {
        let mut directories = Vec::new();
        if let Some(parent) = self.current_exe.as_deref().and_then(Path::parent) {
            let parent = if parent.file_name().is_some_and(|name| name == "deps") {
                parent.parent().unwrap_or(parent)
            } else {
                parent
            };
            directories.push(parent.to_path_buf());
        }
        directories.push(self.root.join("scripts/rust/target/release"));
        if !self.home.as_os_str().is_empty() {
            directories.push(self.home.join("dotfiles/.bin"));
        }
        directories.push(self.root.join("scripts/rust/target/debug"));
        let mut paths = Vec::new();
        for directory in directories {
            let path = directory.join(name);
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        paths
    }
}

fn binary_name(program: &str) -> &str {
    if program == "gdd" {
        "git-discard"
    } else {
        program
    }
}

pub fn is_executable(path: &Path) -> Result<bool, String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("inspect {}: {error}", path.display())),
    };
    if !metadata.is_file() {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Ok(false);
        }
    }
    let mut magic = [0u8; 4];
    let mut file =
        fs::File::open(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    if let Err(error) = file.read_exact(&mut magic) {
        return if error.kind() == std::io::ErrorKind::UnexpectedEof {
            Ok(false)
        } else {
            Err(format!("read {}: {error}", path.display()))
        };
    }
    Ok(matches!(
        magic,
        [0x7f, b'E', b'L', b'F']
            | [0xfe, 0xed, 0xfa, 0xce | 0xcf]
            | [0xce | 0xcf, 0xfa, 0xed, 0xfe]
            | [0xca, 0xfe, 0xba, 0xbe | 0xbf]
            | [0xbe | 0xbf, 0xba, 0xfe, 0xca]
            | [b'M', b'Z', _, _]
    ))
}
