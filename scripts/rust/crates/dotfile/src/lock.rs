use std::fs::{self, File, OpenOptions};
use std::io::{Seek, Write};
use std::path::Path;

use crate::context::Context;

pub struct SyncLock {
    #[cfg(unix)]
    _file: nix::fcntl::Flock<File>,
    #[cfg(not(unix))]
    _file: File,
}

impl SyncLock {
    pub fn acquire(state: &Path) -> Result<Self, String> {
        fs::create_dir_all(state).map_err(|e| format!("{}: {e}", state.display()))?;
        Self::at(&state.join("sync.lock"))
    }

    fn at(path: &Path) -> Result<Self, String> {
        Self::held(path, false, &|| {})
    }

    fn held(path: &Path, wait: bool, announce: &dyn Fn()) -> Result<Self, String> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(path)
            .map_err(|e| format!("lock {}: {e}", path.display()))?;
        #[cfg(unix)]
        let mut file = {
            use nix::fcntl::{Flock, FlockArg};
            match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
                Ok(file) => file,
                Err((file, nix::errno::Errno::EWOULDBLOCK)) if wait => {
                    announce();
                    Flock::lock(file, FlockArg::LockExclusive)
                        .map_err(|(_, e)| format!("lock {}: {e}", path.display()))?
                }
                Err((_, nix::errno::Errno::EWOULDBLOCK)) => {
                    return Err("another dotfile mutation is already running".to_string());
                }
                Err((_, e)) => return Err(format!("lock {}: {e}", path.display())),
            }
        };
        #[cfg(not(unix))]
        let mut file = file;
        file.set_len(0)
            .map_err(|e| format!("lock {}: {e}", path.display()))?;
        file.rewind().map_err(|e| e.to_string())?;
        writeln!(file, "{}", std::process::id()).map_err(|e| e.to_string())?;
        Ok(Self { _file: file })
    }
}

/// Serializes toolchain installation. Unlike a mutation, a second setup wants
/// exactly the result the first is producing, so it waits instead of failing.
pub struct SetupLock {
    _state: SyncLock,
}

impl SetupLock {
    pub fn acquire(context: &Context) -> Result<Self, String> {
        fs::create_dir_all(&context.root_config)
            .map_err(|e| format!("{}: {e}", context.root_config.display()))?;
        Ok(Self {
            _state: SyncLock::held(&context.root_config.join("setup.lock"), true, &|| {
                println!("another setup is running; waiting");
            })?,
        })
    }
}

/// Advisory lock files stay in place: unlinking would let waiters lock different inodes.
pub struct MutationLock {
    _repository: Option<SyncLock>,
    _state: SyncLock,
}

impl MutationLock {
    pub fn acquire(context: &Context) -> Result<Self, String> {
        let git = context.root.join(".git");
        let git = if git.is_file() {
            let text =
                fs::read_to_string(&git).map_err(|e| format!("read {}: {e}", git.display()))?;
            let relative = text
                .trim()
                .strip_prefix("gitdir: ")
                .ok_or("invalid .git file")?;
            context.root.join(relative)
        } else {
            git
        };
        let repository = if git.is_dir() {
            let common = git.join("commondir");
            let directory = if common.is_file() {
                git.join(
                    fs::read_to_string(&common)
                        .map_err(|e| e.to_string())?
                        .trim(),
                )
            } else {
                git
            };
            Some(SyncLock::at(&directory.join("dotfile.lock"))?)
        } else {
            None
        };
        let lock = Self {
            _repository: repository,
            _state: SyncLock::acquire(&context.root_config)?,
        };
        crate::fs::transaction::recover(context)?;
        Ok(lock)
    }
}
