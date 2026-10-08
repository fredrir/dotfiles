use std::os::unix::net::UnixListener;

use super::*;

#[test]
fn the_named_display_wins_when_it_answers() {
    let runtime = tempfile::tempdir().unwrap();
    let _old = UnixListener::bind(runtime.path().join("wayland-0")).unwrap();
    let _named = UnixListener::bind(runtime.path().join("wayland-1")).unwrap();
    let found = wayland_socket(runtime.path(), Some(OsStr::new("wayland-1")));
    assert_eq!(found.as_deref(), Some(OsStr::new("wayland-1")));
}

#[test]
fn a_stale_named_display_falls_back_to_a_live_socket() {
    let runtime = tempfile::tempdir().unwrap();
    let _live = UnixListener::bind(runtime.path().join("wayland-0")).unwrap();
    drop(UnixListener::bind(runtime.path().join("wayland-9")).unwrap());
    fs::write(runtime.path().join("wayland-0.lock"), "").unwrap();
    let found = wayland_socket(runtime.path(), Some(OsStr::new("wayland-9")));
    assert_eq!(found.as_deref(), Some(OsStr::new("wayland-0")));
}

#[test]
fn no_live_socket_means_no_session() {
    let runtime = tempfile::tempdir().unwrap();
    drop(UnixListener::bind(runtime.path().join("wayland-0")).unwrap());
    fs::write(runtime.path().join("wayland-1"), "").unwrap();
    assert_eq!(wayland_socket(runtime.path(), None), None);
    assert_eq!(wayland_socket(&runtime.path().join("missing"), None), None);
}
