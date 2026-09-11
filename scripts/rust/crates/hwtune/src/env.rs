use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub struct Sysfs {
    pub sys: PathBuf,
    pub dev: PathBuf,
}

impl Sysfs {
    pub fn from_env() -> Self {
        Self {
            sys: env::var_os("HWTUNE_SYSFS_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/sys")),
            dev: env::var_os("HWTUNE_DEV_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/dev")),
        }
    }

    pub fn hwmon(&self, name: &str) -> Result<PathBuf, String> {
        let class = self.sys.join("class").join("hwmon");
        let entries = fs::read_dir(&class).map_err(|e| format!("{}: {e}", class.display()))?;
        for entry in entries.flatten() {
            let dir = entry.path();
            if read_text(&dir.join("name")).is_ok_and(|found| found == name) {
                return Ok(dir);
            }
        }
        Err(format!("hwmon device {name} not found"))
    }
}

pub fn read_text(path: &Path) -> Result<String, String> {
    fs::read_to_string(path)
        .map(|text| text.trim().to_string())
        .map_err(|e| format!("{}: {e}", path.display()))
}

pub fn read_number(path: &Path) -> Result<u64, String> {
    let text = read_text(path)?;
    text.parse()
        .map_err(|_| format!("{}: not a number: {text}", path.display()))
}
