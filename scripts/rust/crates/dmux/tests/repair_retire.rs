//! `dmux repair retire-incarnation` end to end (ADR 012 WS-B.3; plan §5.2
//! instance state F): the operator's explicit clear for a published
//! incarnation whose process is gone. One compare-and-set on the published
//! epoch, journaled; every refusal is typed and mutates nothing.

use std::process::{Command, Output, Stdio};

use dmux::backend::scope::{ManagedTarget, resolve_managed};
use dmux::model::{Backend, ServerEpoch};
use dmux::registry::{Registry, RegistryConfig};
use serde_json::Value;
use uuid::Uuid;

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("data")).unwrap();
        std::fs::create_dir_all(dir.path().join("locks")).unwrap();
        Home { dir }
    }

    fn registry(&self) -> Registry {
        Registry::open(RegistryConfig::new(
            self.dir.path().join("data/registry.sqlite3"),
            self.dir.path().join("locks"),
        ))
        .unwrap()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dmux"));
        command
            .args(["--format", "json", "repair", "retire-incarnation"])
            .args(args)
            .args(["--data-dir", self.dir.path().join("data").to_str().unwrap()])
            .args([
                "--lock-dir",
                self.dir.path().join("locks").to_str().unwrap(),
            ])
            .env("DMUX_RUNTIME_DIR", self.dir.path().join("locks"))
            .env_remove("DMUX_WEZ_FIRST")
            .stdin(Stdio::null());
        command
    }

    fn retire(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("dmux runs")
    }

    /// The same invocation under the Wez-first flag, which is what lets an
    /// enrolled alias/label/UID on the global `--host` reach the verb at all
    /// (flag-off, `main` refuses every non-legacy host before dispatch).
    fn retire_wez_first(&self, args: &[&str]) -> Output {
        self.command(args)
            .env("DMUX_WEZ_FIRST", "1")
            .output()
            .expect("dmux runs")
    }
}

fn document(out: &Output) -> Value {
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(text.lines().count(), 1, "not one document: {text:?}");
    serde_json::from_str(text.trim()).unwrap_or_else(|e| panic!("{e}: {text}"))
}

/// A published tmux incarnation naming a pid that no longer exists.
fn seed_dead_incarnation(home: &Home) -> (ServerEpoch, i64) {
    let mut registry = home.registry();
    let instance = registry
        .register_backend_instance(Backend::Tmux, Some("dmux-retire-test"), None)
        .unwrap();
    let epoch = ServerEpoch(Uuid::new_v4());
    // A pid in the range the kernel hands out but that nothing holds now:
    // spawn and reap a child, then use its pid.
    let child = Command::new("true").spawn().unwrap();
    let dead_pid = i64::from(child.id());
    let _ = child.wait_with_output();
    registry
        .publish_backend_server(instance, epoch, Some(dead_pid), Some("gone"), None, None)
        .unwrap();
    (epoch, dead_pid)
}

#[test]
fn retire_incarnation_clears_only_the_named_epoch_after_confirmation() {
    let home = Home::new();
    let (epoch, dead_pid) = seed_dead_incarnation(&home);
    let epoch_text = epoch.0.to_string();

    // JSON without --yes: the one confirmation document, nothing changed.
    let out = home.retire(&["--backend", "tmux", "--epoch", &epoch_text]);
    assert_eq!(
        out.status.code(),
        Some(5),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc = document(&out);
    assert_eq!(doc["errors"][0]["code"], "confirmation_required", "{doc}");
    assert!(
        matches!(
            resolve_managed(&home.registry(), Backend::Tmux).unwrap(),
            ManagedTarget::StaleIncarnation { .. }
        ),
        "a declined retire changes nothing: the row still publishes the dead incarnation"
    );

    // The wrong epoch refuses as an epoch fault and changes nothing.
    let out = home.retire(&[
        "--backend",
        "tmux",
        "--epoch",
        &Uuid::new_v4().to_string(),
        "--yes",
    ]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(document(&out)["errors"][0]["code"], "backend_epoch_changed");
    assert!(matches!(
        resolve_managed(&home.registry(), Backend::Tmux).unwrap(),
        ManagedTarget::StaleIncarnation { .. }
    ));

    // The named epoch retires: the row is cleared and the chain advanced.
    let before = home.registry().authority_head().unwrap().revision;
    let out = home.retire(&["--backend", "tmux", "--epoch", &epoch_text, "--yes"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc = document(&out);
    assert_eq!(doc["ok"], true, "{doc}");
    assert_eq!(doc["action"], "repair_retire_incarnation", "{doc}");
    assert_eq!(doc["result"]["retired_epoch"], epoch_text.as_str(), "{doc}");
    assert_eq!(doc["result"]["retired_pid"], dead_pid, "{doc}");
    let registry = home.registry();
    assert!(registry.authority_head().unwrap().revision > before);
    assert!(
        matches!(
            resolve_managed(&registry, Backend::Tmux).unwrap(),
            ManagedTarget::Unpublished(_)
        ),
        "a retired instance resolves as unpublished"
    );

    // Nothing is published any more, so a second retire is not-found.
    drop(registry);
    let out = home.retire(&["--backend", "tmux", "--epoch", &epoch_text, "--yes"]);
    assert_eq!(
        out.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(document(&out)["errors"][0]["code"], "not_found");
}

#[test]
fn retire_incarnation_guards_a_live_pid_unless_told_otherwise() {
    let home = Home::new();
    let epoch = ServerEpoch(Uuid::new_v4());
    {
        let mut registry = home.registry();
        let instance = registry
            .register_backend_instance(Backend::Tmux, Some("dmux-retire-live"), None)
            .unwrap();
        // This test process is alive for the duration of the test.
        registry
            .publish_backend_server(
                instance,
                epoch,
                Some(i64::from(std::process::id())),
                Some("live"),
                None,
                None,
            )
            .unwrap();
    }
    let epoch_text = epoch.0.to_string();
    let out = home.retire(&["--backend", "tmux", "--epoch", &epoch_text, "--yes"]);
    assert_eq!(
        out.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(document(&out)["errors"][0]["code"], "repair_required");
    assert!(matches!(
        resolve_managed(&home.registry(), Backend::Tmux).unwrap(),
        ManagedTarget::Managed { .. }
    ));

    let out = home.retire(&[
        "--backend",
        "tmux",
        "--epoch",
        &epoch_text,
        "--yes",
        "--allow-live-pid",
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(matches!(
        resolve_managed(&home.registry(), Backend::Tmux).unwrap(),
        ManagedTarget::Unpublished(_)
    ));
}

/// Retire is owner-local (plan §7.4): the global `--host` naming an enrolled
/// peer is the same typed refusal `rebind` gives (ADR 011 D7), before any
/// confirmation and without touching the row. This host's own spellings —
/// the local alias `a` and its HostUid — are not remote.
#[test]
fn retire_incarnation_refuses_a_foreign_host_as_protocol_mismatch() {
    let home = Home::new();
    let (epoch, _) = seed_dead_incarnation(&home);
    let peer = home
        .registry()
        .enroll_host(dmux::model::HostUid(Uuid::new_v4()), Some("archie"))
        .unwrap();
    let epoch_text = epoch.0.to_string();

    for spelling in [
        peer.alias.clone(),
        "archie".to_string(),
        peer.host_uid.0.to_string(),
    ] {
        let out = home.retire_wez_first(&[
            "--host",
            &spelling,
            "--backend",
            "tmux",
            "--epoch",
            &epoch_text,
            "--yes",
        ]);
        assert_eq!(
            out.status.code(),
            Some(6),
            "{spelling}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let doc = document(&out);
        assert_eq!(
            doc["errors"][0]["code"], "protocol_mismatch",
            "{spelling}: {doc}"
        );
        assert!(
            doc["errors"][0]["message"]
                .as_str()
                .unwrap()
                .contains("owner-local"),
            "{doc}"
        );
        assert!(
            matches!(
                resolve_managed(&home.registry(), Backend::Tmux).unwrap(),
                ManagedTarget::StaleIncarnation { .. }
            ),
            "{spelling}: a refused retire changes nothing"
        );
    }

    // An unknown host is not-found, not a quiet local retirement.
    let out = home.retire_wez_first(&[
        "--host",
        "nowhere",
        "--backend",
        "tmux",
        "--epoch",
        &epoch_text,
        "--yes",
    ]);
    assert_eq!(
        out.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(document(&out)["errors"][0]["code"], "not_found");

    // Naming this host is not remote: the local alias refuses nothing, and
    // the retirement proceeds exactly as without `--host`.
    let local_uid = home.registry().identity().unwrap().host_uid.0.to_string();
    let out = home.retire_wez_first(&[
        "--host",
        &local_uid,
        "--backend",
        "tmux",
        "--epoch",
        &epoch_text,
    ]);
    assert_eq!(
        out.status.code(),
        Some(5),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(document(&out)["errors"][0]["code"], "confirmation_required");
    let out = home.retire_wez_first(&[
        "--host",
        "a",
        "--backend",
        "tmux",
        "--epoch",
        &epoch_text,
        "--yes",
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(matches!(
        resolve_managed(&home.registry(), Backend::Tmux).unwrap(),
        ManagedTarget::Unpublished(_)
    ));
}
