//! POSIX scoped kernel locks and the normative acquisition ordering
//! (plan §10.1).
//!
//! Contract: authority gate (shared; maintenance exclusive) → decision locks
//! in exact-byte lexical order → backend-instance lock(s) by
//! BackendInstanceUid → Space lock; release in reverse; no decision lock
//! after backend/Space. Kernel `fcntl` locks provide non-stealable
//! exclusion; SQLite lease rows (`registry`) record ownership and fencing
//! tokens; clock expiry alone never authorizes takeover.
//!
//! Lock files live under a caller-supplied directory: production callers
//! pass `crate::runtime::dmux_runtime_dir()`, tests pass a scratch dir.
//! Locks are open-file-description (OFD) locks, so two lock handles conflict
//! even inside one process (each `HeldLock` owns its own open description),
//! and the lock dies with the last descriptor of that description — a paused
//! live holder keeps it, and nothing can steal it.

use std::fs::OpenOptions;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::model::{BackendInstanceUid, HostUid, SpaceUid};
use crate::registry::sha256::sha256_hex;

#[cfg(target_os = "linux")]
use libc::{F_OFD_SETLK, F_OFD_SETLKW};
// Public in Apple's <sys/fcntl.h> since macOS 10.12; older libc crate
// versions do not expose them for apple targets, so pin the values.
#[cfg(target_os = "macos")]
const F_OFD_SETLK: libc::c_int = 90;
#[cfg(target_os = "macos")]
const F_OFD_SETLKW: libc::c_int = 91;

const RANK_GATE: u8 = 0;
const RANK_DECISION: u8 = 1;
const RANK_BACKEND: u8 = 2;
const RANK_SPACE: u8 = 3;

/// One kernel-lock scope in the normative §10.1 order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockScope {
    /// Every operation takes this first, shared; maintenance takes it
    /// exclusive and therefore overlaps nothing.
    AuthorityGate,
    /// Durable per-logical-name decision lock:
    /// `decision:<owner>:<sha256-of-exact-name-bytes>`.
    Decision {
        owner: HostUid,
        name_sha256: String,
    },
    /// The common backend-instance lock: inventory shared, mutation (and
    /// recovery/snapshot database scopes) exclusive.
    BackendInstance(BackendInstanceUid),
    Space(SpaceUid),
}

impl LockScope {
    /// The decision scope for one exact, case-sensitive logical name.
    pub fn decision(owner: HostUid, exact_name: &str) -> LockScope {
        LockScope::Decision {
            owner,
            name_sha256: sha256_hex(exact_name.as_bytes()),
        }
    }

    /// Acquisition rank; a later acquisition must never have a lower rank.
    pub fn rank(&self) -> u8 {
        match self {
            LockScope::AuthorityGate => RANK_GATE,
            LockScope::Decision { .. } => RANK_DECISION,
            LockScope::BackendInstance(_) => RANK_BACKEND,
            LockScope::Space(_) => RANK_SPACE,
        }
    }

    /// The canonical scope key. Same-rank scopes are acquired in strictly
    /// increasing exact-byte lexical order of this key; for decision scopes
    /// that is the plan's exact-byte order (the key embeds the name's
    /// sha256), for backend instances it is BackendInstanceUid order
    /// (lowercase hyphenated hex is byte-order-preserving).
    pub fn key(&self) -> String {
        match self {
            LockScope::AuthorityGate => "authority-gate".into(),
            LockScope::Decision { owner, name_sha256 } => {
                format!("decision:{}:{}", owner.0, name_sha256)
            }
            LockScope::BackendInstance(uid) => format!("backend:{}", uid.0),
            LockScope::Space(uid) => format!("space:{}", uid.0),
        }
    }

    /// Stable lock-file name under the runtime dir.
    pub fn file_name(&self) -> String {
        format!("{}.lock", self.key().replace(':', "_"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    Shared,
    Exclusive,
}

/// A held kernel lock. Dropping it closes the open file description, which
/// releases the OFD lock. There is no unlock-without-release path: a lock
/// cannot be stolen, only dropped by its holder or by holder death.
#[derive(Debug)]
pub struct HeldLock {
    _file: std::fs::File,
    scope: LockScope,
    mode: LockMode,
}

impl HeldLock {
    pub fn scope(&self) -> &LockScope {
        &self.scope
    }

    pub fn mode(&self) -> LockMode {
        self.mode
    }
}

/// Standalone single-scope acquisition (blocking). Multi-scope operations
/// must use [`OrderedLocks`]; this exists for single-lock patterns such as
/// the maintenance gate around schema migration.
pub fn acquire(dir: &Path, scope: LockScope, mode: LockMode) -> io::Result<HeldLock> {
    lock_file(dir, scope, mode, true).map(|held| held.expect("blocking acquire returned busy"))
}

/// Standalone single-scope non-blocking acquisition; `None` means a
/// conflicting holder exists.
pub fn try_acquire(dir: &Path, scope: LockScope, mode: LockMode) -> io::Result<Option<HeldLock>> {
    lock_file(dir, scope, mode, false)
}

fn lock_file(
    dir: &Path,
    scope: LockScope,
    mode: LockMode,
    blocking: bool,
) -> io::Result<Option<HeldLock>> {
    let path = dir.join(scope.file_name());
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)?;
    if fcntl_lock(&file, mode, blocking)? {
        Ok(Some(HeldLock {
            _file: file,
            scope,
            mode,
        }))
    } else {
        Ok(None)
    }
}

fn fcntl_lock(file: &std::fs::File, mode: LockMode, blocking: bool) -> io::Result<bool> {
    let mut fl: libc::flock = unsafe { std::mem::zeroed() };
    fl.l_type = match mode {
        LockMode::Shared => libc::F_RDLCK as libc::c_short,
        LockMode::Exclusive => libc::F_WRLCK as libc::c_short,
    };
    fl.l_whence = libc::SEEK_SET as libc::c_short;
    fl.l_start = 0;
    fl.l_len = 0; // whole file
    fl.l_pid = 0; // required 0 for OFD locks
    let cmd = if blocking { F_OFD_SETLKW } else { F_OFD_SETLK };
    loop {
        let rc = unsafe { libc::fcntl(file.as_raw_fd(), cmd, &fl as *const libc::flock) };
        if rc == 0 {
            return Ok(true);
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::EAGAIN) | Some(libc::EACCES) if !blocking => return Ok(false),
            _ => return Err(err),
        }
    }
}

#[derive(Debug)]
pub enum LockError {
    /// Acquisition violating the normative §10.1 order. In debug builds
    /// this panics instead (the violation is a programming error).
    OutOfOrder {
        held: Option<String>,
        requested: String,
    },
    Io(io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::OutOfOrder { held, requested } => write!(
                f,
                "kernel lock order violation: {requested} requested while holding {}",
                held.as_deref()
                    .unwrap_or("nothing (authority gate must come first)")
            ),
            LockError::Io(e) => write!(f, "kernel lock i/o: {e}"),
        }
    }
}

impl std::error::Error for LockError {}

impl From<io::Error> for LockError {
    fn from(e: io::Error) -> Self {
        LockError::Io(e)
    }
}

/// Ordered multi-scope acquisition enforcing plan §10.1 at runtime:
///
/// 1. The first acquisition must be the authority gate.
/// 2. Ranks never decrease (so no decision lock after backend/Space).
/// 3. Same-rank scopes are acquired in strictly increasing exact-byte
///    lexical `key()` order (decision locks by name-sha bytes, backend
///    instances by BackendInstanceUid) — and never twice.
/// 4. Release happens in exact reverse acquisition order, including on drop.
///
/// A violation panics in debug builds and returns [`LockError::OutOfOrder`]
/// in release builds.
#[derive(Debug)]
pub struct OrderedLocks {
    dir: PathBuf,
    held: Vec<HeldLock>,
}

impl OrderedLocks {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        OrderedLocks {
            dir: dir.into(),
            held: Vec::new(),
        }
    }

    /// Blocking in-order acquisition.
    pub fn acquire(&mut self, scope: LockScope, mode: LockMode) -> Result<(), LockError> {
        self.check_order(&scope)?;
        let held =
            lock_file(&self.dir, scope, mode, true)?.expect("blocking acquire returned busy");
        self.held.push(held);
        Ok(())
    }

    /// Non-blocking in-order acquisition; `Ok(false)` means a conflicting
    /// holder exists and nothing was acquired.
    pub fn try_acquire(&mut self, scope: LockScope, mode: LockMode) -> Result<bool, LockError> {
        self.check_order(&scope)?;
        match lock_file(&self.dir, scope, mode, false)? {
            Some(held) => {
                self.held.push(held);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Acquire the decision locks for `names` in the plan's exact-byte
    /// lexical order regardless of argument order.
    pub fn acquire_decisions(
        &mut self,
        owner: HostUid,
        names: &[&str],
        mode: LockMode,
    ) -> Result<(), LockError> {
        let mut scopes: Vec<LockScope> = names
            .iter()
            .map(|name| LockScope::decision(owner, name))
            .collect();
        scopes.sort_by(|a, b| a.key().cmp(&b.key()));
        scopes.dedup();
        for scope in scopes {
            self.acquire(scope, mode)?;
        }
        Ok(())
    }

    fn check_order(&self, scope: &LockScope) -> Result<(), LockError> {
        let ok = match self.held.last() {
            None => matches!(scope, LockScope::AuthorityGate),
            Some(last) => {
                let (last_rank, rank) = (last.scope().rank(), scope.rank());
                rank > last_rank
                    || (rank == last_rank && rank != RANK_GATE && scope.key() > last.scope().key())
            }
        };
        if ok {
            return Ok(());
        }
        let held = self.held.last().map(|h| h.scope().key());
        let requested = scope.key();
        if cfg!(debug_assertions) {
            panic!(
                "kernel lock order violation (plan §10.1): {requested} requested while holding {held:?}"
            );
        }
        Err(LockError::OutOfOrder { held, requested })
    }

    /// The held guard for `scope`, e.g. to pair a registry lease with its
    /// kernel lock.
    pub fn held(&self, scope: &LockScope) -> Option<&HeldLock> {
        self.held.iter().find(|h| h.scope() == scope)
    }

    pub fn held_scopes(&self) -> Vec<&LockScope> {
        self.held.iter().map(|h| h.scope()).collect()
    }

    /// Release the most recently acquired lock (reverse order).
    pub fn release_last(&mut self) -> Option<LockScope> {
        self.held.pop().map(|h| h.scope.clone())
    }

    /// Release everything, newest first.
    pub fn release_all(&mut self) {
        while self.held.pop().is_some() {}
    }
}

impl Drop for OrderedLocks {
    fn drop(&mut self) {
        // Vec would drop front-to-back; the contract releases in reverse.
        self.release_all();
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn decision_scope_hashes_exact_name_bytes() {
        let owner = HostUid(Uuid::nil());
        let scope = LockScope::decision(owner, "Project");
        let LockScope::Decision { name_sha256, .. } = &scope else {
            panic!("not a decision scope");
        };
        assert_eq!(name_sha256, &sha256_hex(b"Project"));
        // Case-sensitive: different bytes, different lock.
        assert_ne!(LockScope::decision(owner, "project").key(), scope.key(),);
        assert_eq!(
            scope.key(),
            format!("decision:{}:{}", Uuid::nil(), sha256_hex(b"Project"))
        );
    }

    #[test]
    fn file_names_are_stable_and_shell_safe() {
        let scope = LockScope::AuthorityGate;
        assert_eq!(scope.file_name(), "authority-gate.lock");
        let backend = LockScope::BackendInstance(BackendInstanceUid(Uuid::nil()));
        let name = backend.file_name();
        assert!(name.starts_with("backend_"), "{name}");
        assert!(name.ends_with(".lock"));
        assert!(!name.contains(':'));
    }

    #[test]
    fn ofd_locks_conflict_within_one_process() {
        let dir = dir();
        let a = acquire(dir.path(), LockScope::AuthorityGate, LockMode::Exclusive).unwrap();
        // A second, distinct open description must NOT get the lock: this is
        // what OFD locks add over plain per-process POSIX locks.
        let b = try_acquire(dir.path(), LockScope::AuthorityGate, LockMode::Exclusive).unwrap();
        assert!(b.is_none());
        drop(a);
        let c = try_acquire(dir.path(), LockScope::AuthorityGate, LockMode::Exclusive).unwrap();
        assert!(c.is_some());
    }

    #[test]
    fn shared_holders_coexist_and_block_exclusive() {
        let dir = dir();
        let s1 = acquire(dir.path(), LockScope::AuthorityGate, LockMode::Shared).unwrap();
        let s2 = try_acquire(dir.path(), LockScope::AuthorityGate, LockMode::Shared).unwrap();
        assert!(s2.is_some(), "two shared holders must coexist");
        let ex = try_acquire(dir.path(), LockScope::AuthorityGate, LockMode::Exclusive).unwrap();
        assert!(ex.is_none(), "exclusive must wait for shared holders");
        drop(s1);
        drop(s2);
        let ex = try_acquire(dir.path(), LockScope::AuthorityGate, LockMode::Exclusive).unwrap();
        assert!(ex.is_some());
    }
}
