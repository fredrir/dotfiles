use std::collections::BTreeMap;
use std::fs;

use crate::context::Context;
use crate::event::{Action, Event, VecSink};
use crate::sync::config::Configuration;

use super::synchronize_with_systemd;

struct CancelReset;

impl Drop for CancelReset {
    fn drop(&mut self) {
        crate::cancel::reset();
    }
}

#[test]
fn legacy_systemd_cleanup_is_profile_independent() {
    let _cancel_lock = crate::cancel::test_lock();
    crate::cancel::reset();
    let _cancel_reset = CancelReset;
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("repo");
    let home = temporary.path().join("home");
    fs::create_dir_all(root.join("config")).unwrap();
    fs::create_dir_all(root.join("environment/test")).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(root.join("config/targets.dotfile"), "").unwrap();
    fs::write(root.join("environment/test/manifest"), "shared\n").unwrap();
    let context = Context::new(root, home.clone(), home.join(".state")).unwrap();
    let old_path = home.join(".config/systemd/user/generate-theme.path");
    fs::create_dir_all(old_path.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink("missing", &old_path).unwrap();
    let configuration = Configuration {
        targets: BTreeMap::new(),
        groups: vec!["shared".to_string()],
        active_override_dirs: Vec::new(),
        overrides: BTreeMap::new(),
        packages: Vec::new(),
    };
    let events = VecSink::default();
    let outcome = synchronize_with_systemd(&context, &configuration, true, true, &events).unwrap();
    assert_eq!(outcome.changed, 1);
    assert!(fs::symlink_metadata(&old_path).is_ok());
    assert!(events.events().iter().any(|event| matches!(
        event,
        Event::Item {
            action: Action::Prune,
            path,
            changed: true,
            ..
        } if path == &old_path
    )));
}
