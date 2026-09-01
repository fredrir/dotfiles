use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

pub struct SyncLock {
    path: PathBuf,
    _file: File,
}

impl SyncLock {
    pub fn acquire(state: &Path) -> Result<Self, String> {
        fs::create_dir_all(state).map_err(|error| format!("{}: {error}", state.display()))?;
        let path = state.join("sync.lock");
        for _ in 0..2 {
            match create(&path) {
                Ok(file) => return Ok(Self { path, _file: file }),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    if owner_alive(&path) {
                        return Err("another dotfile sync is already running".to_string());
                    }
                    fs::remove_file(&path)
                        .map_err(|remove| format!("{}: {remove}", path.display()))?;
                }
                Err(error) => return Err(format!("{}: {error}", path.display())),
            }
        }
        Err("could not acquire the dotfile sync lock".to_string())
    }
}

impl Drop for SyncLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn create(path: &Path) -> std::io::Result<File> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    writeln!(file, "{}", std::process::id())?;
    file.sync_data()?;
    Ok(file)
}

fn owner_alive(path: &Path) -> bool {
    let pid = fs::read_to_string(path)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok());
    pid.is_some_and(process_alive)
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as i32, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    true
}
