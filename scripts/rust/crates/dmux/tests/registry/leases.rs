//! Fenced leases (plan §10.1/§10.2): monotonic fencing tokens per scope,
//! mandatory kernel-lock pairing, clock expiry never authorizing takeover,
//! takeover blocked while a live child process holds the kernel lock, and
//! the fencing token advancing on legitimate takeover.

use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use dmux::locks::{self, LockMode, LockScope};
use dmux::registry::{
    HolderLiveness, LeaseHolder, LeaseScope, RegistryError, TakeoverProof, probe_pid,
};
use uuid::Uuid;

use crate::util::{open, scratch, tmux_instance};

#[cfg(target_os = "linux")]
use libc::F_OFD_SETLK;
#[cfg(target_os = "macos")]
const F_OFD_SETLK: libc::c_int = 90;

/// Spawn a child process that holds an exclusive OFD lock on `path`.
///
/// The lock is taken on an open file description BEFORE spawning; the child
/// (`cat` blocked on its piped stdin) inherits that description, and the
/// parent then closes its own descriptor. From that point the lock lives
/// exactly as long as the child does — a real cross-process, non-stealable
/// kernel lock held by a live PID.
fn spawn_lock_holder(path: &Path) -> Child {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .unwrap();
    let mut fl: libc::flock = unsafe { std::mem::zeroed() };
    fl.l_type = libc::F_WRLCK as libc::c_short;
    fl.l_whence = libc::SEEK_SET as libc::c_short;
    fl.l_start = 0;
    fl.l_len = 0;
    fl.l_pid = 0;
    let rc = unsafe { libc::fcntl(file.as_raw_fd(), F_OFD_SETLK, &fl) };
    assert_eq!(rc, 0, "lock: {}", std::io::Error::last_os_error());
    // Clear CLOEXEC so the child inherits the locked description.
    let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, 0) };
    assert_eq!(rc, 0);
    let child = Command::new("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    drop(file); // the child's inherited descriptor keeps the lock alive
    child
}

fn holder(pid: i32) -> LeaseHolder {
    LeaseHolder {
        request_uid: Uuid::new_v4(),
        pid,
        start_token: Uuid::new_v4().to_string(),
        boot_id: None,
    }
}

const TTL: Duration = Duration::from_secs(30);

#[test]
fn fencing_tokens_are_monotonic_per_scope_and_stable_across_renewal() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let scope = LeaseScope::Backend(instance);
    let kernel = locks::acquire(
        &s.config.lock_dir,
        LockScope::BackendInstance(instance),
        LockMode::Exclusive,
    )
    .unwrap();

    let h1 = holder(std::process::id() as i32);
    let l1 = reg.acquire_lease(&scope, &h1, TTL, &kernel, None).unwrap();
    assert_eq!(l1.fencing_token, 1);

    // Renewal and same-request resume keep the token.
    let renewed = reg.renew_lease(&scope, h1.request_uid, TTL).unwrap();
    assert_eq!(renewed.fencing_token, 1);
    let resumed = reg.acquire_lease(&scope, &h1, TTL, &kernel, None).unwrap();
    assert_eq!(resumed.fencing_token, 1);

    // Release then re-acquire: strictly increasing grants.
    reg.release_lease(&scope, h1.request_uid).unwrap();
    let h2 = holder(std::process::id() as i32);
    assert_eq!(
        reg.acquire_lease(&scope, &h2, TTL, &kernel, None)
            .unwrap()
            .fencing_token,
        2
    );
    reg.release_lease(&scope, h2.request_uid).unwrap();
    let h3 = holder(std::process::id() as i32);
    assert_eq!(
        reg.acquire_lease(&scope, &h3, TTL, &kernel, None)
            .unwrap()
            .fencing_token,
        3
    );
    assert_eq!(reg.last_fencing_token(&scope).unwrap(), 3);

    // Scopes count independently (recovery/snapshot pair with the same
    // backend-instance kernel lock).
    let recovery = LeaseScope::Recovery(instance);
    let h4 = holder(std::process::id() as i32);
    assert_eq!(
        reg.acquire_lease(&recovery, &h4, TTL, &kernel, None)
            .unwrap()
            .fencing_token,
        1
    );
}

#[test]
fn lease_requires_its_paired_exclusive_kernel_lock() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let scope = LeaseScope::Backend(instance);
    let h = holder(std::process::id() as i32);

    // Shared mode is not mutation authority.
    let shared = locks::acquire(
        &s.config.lock_dir,
        LockScope::BackendInstance(instance),
        LockMode::Shared,
    )
    .unwrap();
    let err = reg
        .acquire_lease(&scope, &h, TTL, &shared, None)
        .unwrap_err();
    assert!(matches!(err, RegistryError::KernelLockMismatch { .. }));
    drop(shared);

    // The wrong kernel scope does not pair either.
    let wrong = locks::acquire(
        &s.config.lock_dir,
        LockScope::AuthorityGate,
        LockMode::Exclusive,
    )
    .unwrap();
    let err = reg
        .acquire_lease(&scope, &h, TTL, &wrong, None)
        .unwrap_err();
    assert!(matches!(err, RegistryError::KernelLockMismatch { .. }));
    // ...though the exclusive gate is exactly what Maintenance pairs with.
    reg.acquire_lease(&LeaseScope::Maintenance, &h, TTL, &wrong, None)
        .unwrap();
}

#[test]
fn clock_expiry_alone_never_authorizes_takeover() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let scope = LeaseScope::Backend(instance);
    let kernel = locks::acquire(
        &s.config.lock_dir,
        LockScope::BackendInstance(instance),
        LockMode::Exclusive,
    )
    .unwrap();

    // The holder is this very process, with an already-expired lease.
    let h1 = holder(std::process::id() as i32);
    reg.acquire_lease(&scope, &h1, Duration::from_millis(1), &kernel, None)
        .unwrap();
    std::thread::sleep(Duration::from_millis(10));

    // A different holder without proof is refused...
    let h2 = holder(std::process::id() as i32);
    let err = reg
        .acquire_lease(&scope, &h2, TTL, &kernel, None)
        .unwrap_err();
    assert!(matches!(err, RegistryError::LeaseHeld { .. }));

    // ...and an honest liveness probe of the (alive) prior holder still
    // refuses: expiry alone is never enough.
    let proof = TakeoverProof {
        prior_pid: h1.pid,
        liveness: probe_pid(h1.pid),
    };
    assert_eq!(proof.liveness, HolderLiveness::Alive);
    let err = reg
        .acquire_lease(&scope, &h2, TTL, &kernel, Some(&proof))
        .unwrap_err();
    let RegistryError::LeaseHeld { holder_pid, .. } = err else {
        panic!("expected LeaseHeld, got {err:?}");
    };
    assert_eq!(holder_pid, Some(h1.pid));
}

#[test]
fn takeover_blocked_by_live_child_then_fencing_advances_on_legitimate_takeover() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let scope = LeaseScope::Backend(instance);
    let kernel_scope = LockScope::BackendInstance(instance);

    // A child process holds the backend-instance kernel lock, exactly like
    // a paused live holder would (plan §10.2: it cannot be timed out).
    let lock_path = s.config.lock_dir.join(kernel_scope.file_name());
    let mut child = spawn_lock_holder(&lock_path);
    let child_pid = child.id() as i32;
    assert_eq!(probe_pid(child_pid), HolderLiveness::Alive);

    // Its persisted lease row (as the crashed/paused holder left it),
    // fencing token 1.
    let prior_request = Uuid::new_v4();
    reg.raw_connection()
        .execute_batch(&format!(
            "INSERT INTO lease_scopes (scope, last_fencing_token) VALUES ('{scope}', 1); \
             INSERT INTO leases (scope, holder_request_uid, fencing_token, holder_pid, \
              holder_start_token, expires_at, renewed_at, state) \
             VALUES ('{scope}', '{prior_request}', 1, {child_pid}, 'tok', \
              '2020-01-01T00:00:00Z', '2020-01-01T00:00:00Z', 'held');",
            scope = scope.as_scope_string(),
        ))
        .unwrap();

    // Takeover is blocked at the kernel: the non-stealable lock is held by
    // a live process, so the would-be successor cannot even reach the lease.
    assert!(
        locks::try_acquire(
            &s.config.lock_dir,
            kernel_scope.clone(),
            LockMode::Exclusive
        )
        .unwrap()
        .is_none(),
        "kernel lock must be unavailable while the holder lives"
    );

    // The holder dies (diagnosed and terminated, not timed out).
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(probe_pid(child_pid), HolderLiveness::Dead);

    // Now the kernel lock is acquirable, the death is provable, and the
    // takeover advances the fencing token.
    let kernel = locks::try_acquire(&s.config.lock_dir, kernel_scope, LockMode::Exclusive)
        .unwrap()
        .expect("kernel lock must be free after the holder died");
    let successor = holder(std::process::id() as i32);
    let proof = TakeoverProof {
        prior_pid: child_pid,
        liveness: probe_pid(child_pid),
    };
    let lease = reg
        .acquire_lease(&scope, &successor, TTL, &kernel, Some(&proof))
        .unwrap();
    assert_eq!(lease.fencing_token, 2, "fencing token advances on takeover");
    assert_eq!(lease.holder_request_uid, successor.request_uid);

    // The predecessor's row is superseded, never deleted.
    let prior_state: String = reg
        .raw_connection()
        .query_row(
            "SELECT state FROM leases WHERE holder_request_uid = ?1",
            [prior_request.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(prior_state, "superseded");
    // And the current lease is the successor's.
    let current = reg.current_lease(&scope).unwrap().unwrap();
    assert_eq!(current.holder_request_uid, successor.request_uid);
    assert_eq!(current.fencing_token, 2);
}

#[test]
fn wrong_pid_in_takeover_proof_is_refused() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let scope = LeaseScope::Backend(instance);
    let kernel = locks::acquire(
        &s.config.lock_dir,
        LockScope::BackendInstance(instance),
        LockMode::Exclusive,
    )
    .unwrap();

    let h1 = holder(std::process::id() as i32);
    reg.acquire_lease(&scope, &h1, TTL, &kernel, None).unwrap();

    // A proof about a DIFFERENT pid (even a dead one) proves nothing about
    // the recorded holder.
    let h2 = holder(std::process::id() as i32);
    let err = reg
        .acquire_lease(
            &scope,
            &h2,
            TTL,
            &kernel,
            Some(&TakeoverProof {
                prior_pid: 99_999_999,
                liveness: HolderLiveness::Dead,
            }),
        )
        .unwrap_err();
    assert!(matches!(err, RegistryError::LeaseHeld { .. }));
}
