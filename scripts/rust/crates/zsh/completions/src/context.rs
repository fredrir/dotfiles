use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct Context {
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub now: SystemTime,
    // Refreshes run in the foreground instead of in a detached process.
    pub foreground: bool,
    pub offline: bool,
    vars: BTreeMap<String, String>,
}

impl Context {
    pub fn from_env() -> Context {
        let vars: BTreeMap<String, String> = std::env::vars().collect();
        let cwd = vars
            .get("PWD")
            .map(PathBuf::from)
            .filter(|pwd| pwd.is_absolute())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/"));
        Context::new(cwd, vars)
    }

    pub fn new(cwd: PathBuf, vars: BTreeMap<String, String>) -> Context {
        let home = vars
            .get("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let flag = |name: &str| vars.get(name).is_some_and(|value| value == "1");
        Context {
            foreground: flag("ZCOMP_FOREGROUND"),
            offline: flag("ZCOMP_OFFLINE"),
            cwd,
            home,
            now: SystemTime::now(),
            vars,
        }
    }

    // Offline, in the foreground, with HOME and the cache inside `home`.
    #[cfg(test)]
    pub fn testing(home: &Path, cwd: &Path, vars: &[(&str, &str)]) -> Context {
        let mut all: BTreeMap<String, String> = [
            ("HOME", home.to_string_lossy().to_string()),
            (
                "ZCOMP_CACHE_DIR",
                home.join(".zcomp").to_string_lossy().to_string(),
            ),
            ("ZCOMP_OFFLINE", "1".into()),
            ("ZCOMP_FOREGROUND", "1".into()),
            ("PATH", String::new()),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect();
        for (name, value) in vars {
            all.insert(name.to_string(), value.to_string());
        }
        Context::new(cwd.to_path_buf(), all)
    }

    pub fn var(&self, name: &str) -> Option<&str> {
        self.vars
            .get(name)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }

    pub fn var_path(&self, name: &str) -> Option<PathBuf> {
        self.var(name).map(|value| self.expand(value))
    }

    pub fn expand(&self, path: &str) -> PathBuf {
        match path.strip_prefix("~/") {
            Some(rest) => self.home.join(rest),
            None if path == "~" => self.home.clone(),
            None => self.cwd.join(path),
        }
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.var_path("ZCOMP_CACHE_DIR")
            .or_else(|| self.var_path("XDG_CACHE_HOME").map(|dir| dir.join("zcomp")))
            .unwrap_or_else(|| self.home.join(".cache/zcomp"))
    }

    pub fn which(&self, program: &str) -> Option<PathBuf> {
        let path = self.var("PATH")?;
        path.split(':')
            .filter(|dir| !dir.is_empty())
            .map(|dir| Path::new(dir).join(program))
            .find(|candidate| is_executable(candidate))
    }

    pub fn now_secs(&self) -> u64 {
        secs(self.now)
    }

    pub fn ancestors(&self) -> impl Iterator<Item = &Path> {
        self.cwd.ancestors()
    }
}

pub fn secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

pub fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

// Changes whenever the file is replaced or rewritten, without reading it.
pub fn fingerprint(path: &Path) -> String {
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    match fs::metadata(&resolved) {
        Ok(meta) => {
            let modified = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |elapsed| elapsed.as_nanos());
            format!("{}:{}:{}", resolved.display(), modified, meta.len())
        }
        Err(_) => format!("{}:missing", resolved.display()),
    }
}

pub fn age(now: u64, then: u64) -> String {
    let elapsed = now.saturating_sub(then);
    match elapsed {
        0..60 => "just now".into(),
        60..3_600 => format!("{}m ago", elapsed / 60),
        3_600..86_400 => format!("{}h ago", elapsed / 3_600),
        86_400..2_592_000 => format!("{}d ago", elapsed / 86_400),
        _ => format!("{}mo ago", elapsed / 2_592_000),
    }
}

#[cfg(test)]
#[path = "../tests/unit/context_tests.rs"]
mod tests;
