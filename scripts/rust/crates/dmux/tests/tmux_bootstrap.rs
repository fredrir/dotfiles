//! P5 gate: the tmux server-epoch bootstrap end to end (plan §11.2).
//! Root-owned. Direct invocation, hook-driven invocation from the managed
//! conf template, idempotence, and restart-invalidates-epoch.

use std::process::Command;
use std::time::{Duration, Instant};

use dmux::model::Backend;
use dmux::registry::{Registry, RegistryConfig};

struct Scratch {
    ns: String,
    data: tempfile::TempDir,
    locks: tempfile::TempDir,
}

impl Scratch {
    fn new(tag: &str) -> Scratch {
        Scratch {
            ns: format!("dmux-p5b-{tag}-{}", std::process::id()),
            data: tempfile::tempdir().unwrap(),
            locks: tempfile::tempdir().unwrap(),
        }
    }

    fn tmux(&self, args: &[&str]) -> std::process::Output {
        Command::new("tmux")
            .args(["-L", &self.ns])
            .args(args)
            .output()
            .unwrap()
    }

    fn epoch_option(&self) -> String {
        String::from_utf8(
            self.tmux(&["show-options", "-gqv", "@dmux_server_epoch"])
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string()
    }

    fn bootstrap(&self) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_dmux"))
            .args([
                "_tmux-bootstrap",
                "--namespace",
                &self.ns,
                "--data-dir",
                self.data.path().to_str().unwrap(),
                "--lock-dir",
                self.locks.path().to_str().unwrap(),
            ])
            .env_remove("TMUX")
            .output()
            .unwrap()
    }

    fn registry_epoch(&self) -> Option<String> {
        let mut registry = Registry::open(RegistryConfig::new(
            self.data.path().join("registry.sqlite3"),
            self.locks.path(),
        ))
        .ok()?;
        // register_backend_instance is idempotent: it returns the existing
        // instance uid for (owner, backend) rather than allocating twice.
        let instance = registry
            .register_backend_instance(Backend::Tmux, Some(&self.ns), None)
            .ok()?;
        registry
            .backend_server(instance)
            .ok()?
            .server_epoch
            .map(|e| e.0.to_string())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = self.tmux(&["kill-server"]);
    }
}

#[test]
fn direct_bootstrap_is_silent_idempotent_and_published() {
    let s = Scratch::new("direct");
    assert!(
        s.tmux(&["-f", "/dev/null", "new-session", "-d", "-s", "seed"])
            .status
            .success()
    );

    let out = s.bootstrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.is_empty(), "hook cleanliness: silent on success");
    let epoch = s.epoch_option();
    assert!(!epoch.is_empty());
    assert_eq!(s.registry_epoch().as_deref(), Some(epoch.as_str()));

    // Idempotent: second run leaves the same epoch bound.
    let again = s.bootstrap();
    assert!(again.status.success());
    assert_eq!(s.epoch_option(), epoch);
    assert_eq!(s.registry_epoch().as_deref(), Some(epoch.as_str()));
}

#[test]
fn restart_invalidates_the_epoch_and_rebinds() {
    let s = Scratch::new("restart");
    assert!(
        s.tmux(&["-f", "/dev/null", "new-session", "-d", "-s", "one"])
            .status
            .success()
    );
    assert!(s.bootstrap().status.success());
    let first = s.epoch_option();

    assert!(s.tmux(&["kill-server"]).status.success());
    assert!(
        s.tmux(&["-f", "/dev/null", "new-session", "-d", "-s", "two"])
            .status
            .success()
    );
    assert_eq!(s.epoch_option(), "", "a fresh incarnation has no epoch");

    assert!(s.bootstrap().status.success());
    let second = s.epoch_option();
    assert!(!second.is_empty());
    assert_ne!(first, second, "restart must mint a different epoch");
    assert_eq!(s.registry_epoch().as_deref(), Some(second.as_str()));
}

#[test]
fn hook_driven_bootstrap_from_the_managed_conf_template() {
    let s = Scratch::new("hook");
    // Generate a concrete conf from the template, as provisioning would.
    let template =
        std::fs::read_to_string("/Users/fredrir/dotfiles/shared/tmux/dmux-managed.conf").unwrap();
    let hook_cmd = format!(
        "{} _tmux-bootstrap --namespace {} --data-dir {} --lock-dir {}",
        env!("CARGO_BIN_EXE_dmux"),
        s.ns,
        s.data.path().display(),
        s.locks.path().display(),
    );
    let conf = template
        .replace("@DMUX@ _tmux-bootstrap", &hook_cmd)
        .replace("@DMUX@", env!("CARGO_BIN_EXE_dmux"));
    let conf_path = s.data.path().join("managed.conf");
    std::fs::write(&conf_path, conf).unwrap();

    assert!(
        s.tmux(&[
            "-f",
            conf_path.to_str().unwrap(),
            "new-session",
            "-d",
            "-s",
            "a"
        ])
        .status
        .success()
    );
    // run-shell -b is asynchronous: poll for the stamped epoch.
    let deadline = Instant::now() + Duration::from_secs(10);
    let epoch = loop {
        let value = s.epoch_option();
        if !value.is_empty() {
            break value;
        }
        assert!(Instant::now() < deadline, "hook never stamped the epoch");
        std::thread::sleep(Duration::from_millis(50));
    };
    // Registry publication follows the stamp; poll briefly for it too.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if s.registry_epoch().as_deref() == Some(epoch.as_str()) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "hook never published the binding"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // Managed-conf invariants from ADR 004/005 hold on the live server.
    let passthrough =
        String::from_utf8(s.tmux(&["show-options", "-gv", "allow-passthrough"]).stdout).unwrap();
    assert_eq!(passthrough.trim(), "all");
    // A second session on the running server leaves the binding unchanged.
    assert!(s.tmux(&["new-session", "-d", "-s", "b"]).status.success());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(s.epoch_option(), epoch);
    assert_eq!(s.registry_epoch().as_deref(), Some(epoch.as_str()));
}
