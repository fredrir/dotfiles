//! P8a gate: local hierarchy end to end on a real scratch tmux server —
//! child creation through the journaled bootstrap broker (real helper),
//! marker propagation into the exec'd program, epoch-qualified refs,
//! cascade refusals, acknowledgement-loss replay, stale-epoch rejection,
//! adopted-space child blocking, child-orphan classification, and
//! `_context` revalidation. Root-owned.

use std::process::Command;
use std::time::{Duration, Instant};

use dmux::backend::tmux::TmuxProvider;
use dmux::backend::{InventoryScope, SplitDirection};
use dmux::bootstrap;
use dmux::model::{Backend, ChildKind, ServerEpoch};
use dmux::operations::{
    CreateRequest, GroupNewRequest, OpError, OperationEnv, SplitNewRequest, adopt_tmux,
    context_read, create_space, group_new, group_remove, group_rename, hierarchy, split_new,
    split_remove, tmux_bootstrap,
};
use dmux::refs::{ChildRefShape, parse_ref};
use uuid::Uuid;

struct Scratch {
    ns: String,
    data: tempfile::TempDir,
    locks: tempfile::TempDir,
}

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let s = Scratch {
            ns: format!("dmux-p8h-{tag}-{}", std::process::id()),
            data: tempfile::tempdir().unwrap(),
            locks: tempfile::tempdir().unwrap(),
        };
        let out = Command::new("tmux")
            .args([
                "-L",
                &s.ns,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "seed",
            ])
            .env("DMUX_RUNTIME_DIR", s.locks.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        s
    }

    fn env(&self) -> OperationEnv {
        OperationEnv {
            db_path: self.data.path().join("registry.sqlite3"),
            lock_dir: self.locks.path().to_path_buf(),
        }
    }

    fn epoch(&self) -> ServerEpoch {
        match tmux_bootstrap(&self.env(), &self.ns).unwrap() {
            dmux::operations::TmuxBootstrapOutcome::Bootstrapped { epoch }
            | dmux::operations::TmuxBootstrapOutcome::AlreadyBound { epoch }
            | dmux::operations::TmuxBootstrapOutcome::Rebound { epoch, .. } => epoch,
        }
    }

    fn scope(&self, epoch: ServerEpoch) -> InventoryScope {
        InventoryScope {
            backend: Backend::Tmux,
            endpoint: self.ns.clone(),
            expected_epoch: Some(epoch),
        }
    }

    fn tmux(&self, args: &[&str]) -> String {
        let out = Command::new("tmux")
            .args(["-L", &self.ns])
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.ns, "kill-server"])
            .output();
    }
}

fn marker_program(marker: &std::path::Path) -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        format!(
            "printf %s \"$DMUX_GROUP_REF|$DMUX_SPLIT_REF|$DMUX_SPACE_UID\" > {} \
             && exec sleep 300",
            marker.display()
        ),
    ]
}

fn wait_marker(marker: &std::path::Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(marker)
            && !text.is_empty()
        {
            return text;
        }
        assert!(Instant::now() < deadline, "helper never exec'd the program");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn helper() -> String {
    env!("CARGO_BIN_EXE_pane-bootstrap").to_string()
}

fn child_shape(child_ref: &str) -> ChildRefShape {
    parse_ref(&format!("x/{child_ref}"))
        .expect("child ref parses")
        .child
        .expect("has child")
}

/// Create one managed Space named `name` and return it.
fn seed_space(s: &Scratch, epoch: ServerEpoch, name: &str) -> dmux::operations::CreatedSpace {
    let provider = TmuxProvider::new(s.ns.clone());
    create_space(
        &s.env(),
        &provider,
        &s.scope(epoch),
        Backend::Tmux,
        &CreateRequest {
            request_uid: Uuid::new_v4(),
            name: name.into(),
            cwd: None,
            program: vec!["sh".into(), "-c".into(), "exec sleep 300".into()],
            helper_bin: helper(),
        },
    )
    .unwrap()
}

#[test]
fn local_hierarchy_full_cycle_with_markers_and_cascade_refusals() {
    let s = Scratch::new("cycle");
    let epoch = s.epoch();
    let scope = s.scope(epoch);
    let provider = TmuxProvider::new(s.ns.clone());
    let created = seed_space(&s, epoch, "proj");

    // --- group new: real helper, marker env proven in the exec'd program.
    let gmark = s.data.path().join("gmark");
    let group = group_new(
        &s.env(),
        &provider,
        &scope,
        &GroupNewRequest {
            request_uid: Uuid::new_v4(),
            space_uid: created.space_uid,
            cwd: Some("/tmp".into()),
            program: marker_program(&gmark),
            helper_bin: helper(),
        },
    )
    .unwrap();
    assert_eq!(group.kind, ChildKind::Group);
    assert_ne!(group.group_ref, created.group_ref, "a NEW group");
    let stamped = wait_marker(&gmark);
    assert_eq!(
        stamped,
        format!(
            "{}|{}|{}",
            group.group_ref, group.split_ref, created.space_uid.0
        ),
        "marker propagation into the child pane"
    );

    // --- split new in the new group, with placement.
    let smark = s.data.path().join("smark");
    let split = split_new(
        &s.env(),
        &provider,
        &scope,
        &SplitNewRequest {
            request_uid: Uuid::new_v4(),
            space_uid: created.space_uid,
            group: child_shape(&group.group_ref),
            direction: SplitDirection::Right,
            percent: Some(30),
            cwd: None,
            program: marker_program(&smark),
            helper_bin: helper(),
        },
    )
    .unwrap();
    assert_eq!(split.kind, ChildKind::Split);
    assert_eq!(split.group_ref, group.group_ref, "split lands in its group");
    assert_ne!(split.split_ref, group.split_ref);
    wait_marker(&smark);
    // cwd inheritance (§11.3): no explicit cwd, so the target split's cwd.
    assert_eq!(split.cwd_source, dmux::operations::CwdSource::TargetSplit);

    // --- hierarchy read: 2 groups; the new group holds 2 splits.
    let tree = hierarchy(&s.env(), &provider, &scope, created.space_uid).unwrap();
    assert_eq!(tree.server_epoch, epoch);
    assert_eq!(tree.groups.len(), 2);
    let new_group = tree
        .groups
        .iter()
        .find(|g| g.group_ref == group.group_ref)
        .expect("new group listed");
    assert_eq!(new_group.splits.len(), 2);
    assert!(
        new_group
            .splits
            .iter()
            .any(|x| x.split_ref == split.split_ref)
    );

    // --- group rename is presentation-only and verified.
    group_rename(
        &s.env(),
        &provider,
        &scope,
        created.space_uid,
        &child_shape(&group.group_ref),
        "editor",
        Uuid::new_v4(),
    )
    .unwrap();
    let tree = hierarchy(&s.env(), &provider, &scope, created.space_uid).unwrap();
    assert_eq!(
        tree.groups
            .iter()
            .find(|g| g.group_ref == group.group_ref)
            .unwrap()
            .title
            .as_deref(),
        Some("editor")
    );

    // --- cascade refusals (§7.2): last Split → group rm; last Group → rm.
    let root_group_ref = tree
        .groups
        .iter()
        .find(|g| g.group_ref != group.group_ref)
        .unwrap();
    let err = split_remove(
        &s.env(),
        &provider,
        &scope,
        created.space_uid,
        &child_shape(&root_group_ref.splits[0].split_ref),
        Uuid::new_v4(),
    )
    .unwrap_err();
    assert!(matches!(err, OpError::Refused(_)), "{err}");

    // --- remove the new split, then the (now sole-split) group.
    split_remove(
        &s.env(),
        &provider,
        &scope,
        created.space_uid,
        &child_shape(&split.split_ref),
        Uuid::new_v4(),
    )
    .unwrap();
    group_remove(
        &s.env(),
        &provider,
        &scope,
        created.space_uid,
        &child_shape(&group.group_ref),
        Uuid::new_v4(),
    )
    .unwrap();
    let tree = hierarchy(&s.env(), &provider, &scope, created.space_uid).unwrap();
    assert_eq!(tree.groups.len(), 1, "back to the root group only");

    let err = group_remove(
        &s.env(),
        &provider,
        &scope,
        created.space_uid,
        &child_shape(&tree.groups[0].group_ref),
        Uuid::new_v4(),
    )
    .unwrap_err();
    assert!(matches!(err, OpError::Refused(_)), "last group: {err}");
}

#[test]
fn replay_stale_epoch_and_adopted_blocking() {
    let s = Scratch::new("guard");
    let epoch = s.epoch();
    let scope = s.scope(epoch);
    let provider = TmuxProvider::new(s.ns.clone());
    let created = seed_space(&s, epoch, "proj");

    // --- acknowledgement-loss replay: same request UID, no second window.
    let mark = s.data.path().join("m");
    let req = GroupNewRequest {
        request_uid: Uuid::new_v4(),
        space_uid: created.space_uid,
        cwd: None,
        program: marker_program(&mark),
        helper_bin: helper(),
    };
    let first = group_new(&s.env(), &provider, &scope, &req).unwrap();
    let again = group_new(&s.env(), &provider, &scope, &req).unwrap();
    assert!(again.replayed);
    assert_eq!(again.group_ref, first.group_ref);
    let windows = s.tmux(&["list-windows", "-t", "proj", "-F", "#{window_id}"]);
    assert_eq!(windows.lines().count(), 2, "replay must not spawn again");

    // --- stale epoch rejection (§6.3): a ref minted under another epoch.
    let mut stale = child_shape(&first.group_ref);
    stale.epoch = ServerEpoch(Uuid::from_u128(7));
    let err = split_new(
        &s.env(),
        &provider,
        &scope,
        &SplitNewRequest {
            request_uid: Uuid::new_v4(),
            space_uid: created.space_uid,
            group: stale,
            direction: SplitDirection::Down,
            percent: None,
            cwd: None,
            program: vec![],
            helper_bin: helper(),
        },
    )
    .unwrap_err();
    assert!(matches!(err, OpError::StaleRef(_)), "{err}");

    // --- adopted (unstamped) Spaces block child mutations (§10.3).
    s.tmux(&["new-session", "-d", "-s", "legacy"]);
    let legacy_id = s
        .tmux(&["list-sessions", "-F", "#{session_id} #{session_name}"])
        .lines()
        .find(|l| l.ends_with("legacy"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    let adopted = adopt_tmux(
        &s.env(),
        &provider,
        &scope,
        &legacy_id,
        None,
        Uuid::new_v4(),
    )
    .unwrap();
    let err = group_new(
        &s.env(),
        &provider,
        &scope,
        &GroupNewRequest {
            request_uid: Uuid::new_v4(),
            space_uid: adopted.space_uid,
            cwd: None,
            program: vec![],
            helper_bin: helper(),
        },
    )
    .unwrap_err();
    assert!(matches!(err, OpError::Refused(_)), "unstamped: {err}");

    // §10.3 healing: every live pane acknowledges via `context stamp`, then
    // health flips to healthy and child mutations unlock.
    let legacy_pane = s
        .tmux(&["list-panes", "-t", "legacy", "-F", "#{pane_id}"])
        .trim()
        .to_string();
    let outcome = dmux::operations::context_stamp(
        &s.env(),
        &provider,
        &scope,
        adopted.space_uid,
        &legacy_pane,
    )
    .unwrap();
    assert_eq!(outcome.pending_panes, 0);
    assert_eq!(outcome.health, dmux::model::Health::Healthy);
    let gmark = s.data.path().join("adopted-g");
    group_new(
        &s.env(),
        &provider,
        &scope,
        &GroupNewRequest {
            request_uid: Uuid::new_v4(),
            space_uid: adopted.space_uid,
            cwd: None,
            program: marker_program(&gmark),
            helper_bin: helper(),
        },
    )
    .expect("healed space accepts child mutations");
    wait_marker(&gmark);
}

#[test]
fn context_revalidation_and_child_orphan_classification() {
    let s = Scratch::new("ctx");
    let epoch = s.epoch();
    let scope = s.scope(epoch);
    let provider = TmuxProvider::new(s.ns.clone());
    let created = seed_space(&s, epoch, "proj");

    // --- `_context`: the root split's pane revalidates to exact live refs.
    let pane = s
        .tmux(&["list-panes", "-t", "proj", "-F", "#{pane_id}"])
        .trim()
        .to_string();
    let context = context_read(&s.env(), &provider, &scope, created.space_uid, &pane).unwrap();
    assert_eq!(context.space_uid, created.space_uid);
    assert_eq!(context.server_epoch, epoch);
    assert_eq!(context.group_ref, created.group_ref);
    assert_eq!(context.split_ref, created.split_ref);

    // Unknown pane: typed not-found, never a guessed marker.
    let err = context_read(&s.env(), &provider, &scope, created.space_uid, "%999").unwrap_err();
    assert!(matches!(err, OpError::NotFound(_)), "{err}");

    // --- the `dmux _context` CLI end to end, exactly as the prompt hook
    // runs it: marker env + pane env in, one validated JSON document out.
    let socket = s.tmux(&["display-message", "-p", "#{socket_path}"]);
    let out = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .args([
            "_context",
            "--data-dir",
            s.data.path().to_str().unwrap(),
            "--lock-dir",
            s.locks.path().to_str().unwrap(),
        ])
        .env("DMUX_SPACE_UID", created.space_uid.0.to_string())
        .env("TMUX", format!("{},1,0", socket.trim()))
        .env("TMUX_PANE", &pane)
        .env_remove("WEZTERM_PANE")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["group_ref"], created.group_ref.as_str());
    assert_eq!(doc["split_ref"], created.split_ref.as_str());
    assert_eq!(doc["space_uid"], created.space_uid.0.to_string().as_str());

    // A bogus claimed Space UID in the pane env is refused, not guessed.
    let out = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .args([
            "_context",
            "--data-dir",
            s.data.path().to_str().unwrap(),
            "--lock-dir",
            s.locks.path().to_str().unwrap(),
        ])
        .env("DMUX_SPACE_UID", Uuid::new_v4().to_string())
        .env("TMUX", format!("{},1,0", socket.trim()))
        .env("TMUX_PANE", &pane)
        .env_remove("WEZTERM_PANE")
        .output()
        .unwrap();
    assert!(!out.status.success(), "unknown space must fail typed");
    assert!(out.stdout.is_empty(), "no marker output on failure");

    // --- child-orphan classification (§11.1): a reserved-title pane whose
    // owner crashed before correlation is found by its exact token title;
    // zero panes is confirmed absence.
    let boot_uid = Uuid::new_v4();
    let title = bootstrap::reserved_title(boot_uid);
    s.tmux(&[
        "split-window",
        "-t",
        "proj",
        "--",
        "/bin/sh",
        "-c",
        "sleep 300",
    ]);
    let orphan_pane = s
        .tmux(&["list-panes", "-t", "proj", "-F", "#{pane_id}"])
        .lines()
        .last()
        .unwrap()
        .to_string();
    s.tmux(&["select-pane", "-t", &orphan_pane, "-T", &title]);
    let titled: Vec<String> = s
        .tmux(&[
            "list-panes",
            "-s",
            "-t",
            "proj",
            "-F",
            "#{pane_id} #{pane_title}",
        ])
        .lines()
        .filter(|l| l.ends_with(&title))
        .map(|l| l.split_whitespace().next().unwrap().to_string())
        .collect();
    match bootstrap::classify_orphans(&titled) {
        bootstrap::OrphanScan::ExactlyOne { pane } => assert_eq!(pane, orphan_pane),
        other => panic!("expected exactly one orphan, got {other:?}"),
    }
    s.tmux(&["kill-pane", "-t", &orphan_pane]);
    let titled: Vec<String> = s
        .tmux(&[
            "list-panes",
            "-s",
            "-t",
            "proj",
            "-F",
            "#{pane_id} #{pane_title}",
        ])
        .lines()
        .filter(|l| l.ends_with(&title))
        .map(|l| l.split_whitespace().next().unwrap().to_string())
        .collect();
    assert!(matches!(
        bootstrap::classify_orphans(&titled),
        bootstrap::OrphanScan::ConfirmedAbsent
    ));
}
