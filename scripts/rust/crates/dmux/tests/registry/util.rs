//! Shared scaffolding: every registry lives in a scratch tempdir with an
//! injected lock dir — tests never touch the real registry, runtime, or
//! state directories.

use std::time::Duration;

use dmux::model::{Backend, BackendInstanceUid};
use dmux::registry::{
    BusyPolicy, NativeBindingSpec, NativeKind, Registry, RegistryConfig, SpaceReservation,
};
use uuid::Uuid;

pub struct Scratch {
    /// Keep the tempdir alive for the test's duration.
    pub dir: tempfile::TempDir,
    pub config: RegistryConfig,
}

pub fn scratch() -> Scratch {
    let dir = tempfile::tempdir().unwrap();
    let config = RegistryConfig {
        db_path: dir.path().join("registry.sqlite3"),
        lock_dir: dir.path().join("locks"),
        busy: fast_busy(),
    };
    Scratch { dir, config }
}

/// Contract semantics, test-speed timings: the production default stays at
/// the contract's 5000 ms busy timeout; tests only shrink the waits.
pub fn fast_busy() -> BusyPolicy {
    BusyPolicy {
        busy_timeout: Duration::from_millis(500),
        attempts: 5,
        retry_base: Duration::from_millis(2),
    }
}

pub fn open(config: &RegistryConfig) -> Registry {
    Registry::open(config.clone()).unwrap()
}

pub fn tmux_instance(reg: &mut Registry) -> BackendInstanceUid {
    reg.register_backend_instance(Backend::Tmux, None, None)
        .unwrap()
}

pub fn reserve(reg: &mut Registry, name: &str, instance: BackendInstanceUid) -> SpaceReservation {
    reg.reserve_space(name, instance, Uuid::new_v4()).unwrap()
}

pub fn finalize(reg: &mut Registry, reservation: &SpaceReservation, token: &str) {
    reg.finalize_create(
        reservation.space_uid,
        reservation.operation_uid,
        &NativeBindingSpec {
            native_token: token.to_string(),
            native_kind: NativeKind::TmuxSessionId,
            server_epoch: None,
        },
    )
    .unwrap()
}
