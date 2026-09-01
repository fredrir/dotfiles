use std::fs;
use std::path::PathBuf;

use dotfile_cli::artifacts::packages;
use dotfile_cli::cli::{Resolution, SyncCli};
use dotfile_cli::context::{Context, write_atomic};
use dotfile_cli::decision::{self, Choice, Prompt};
use dotfile_cli::event::{Action, Event, VecSink};
use dotfile_cli::sync::engine;
use tempfile::TempDir;

struct Sandbox {
    _temporary: TempDir,
    root: PathBuf,
    home: PathBuf,
    context: Context,
}

impl Sandbox {
    fn new(manifest: &str, targets: &str) -> Self {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("repo");
        let home = temporary.path().join("home");
        fs::create_dir_all(root.join("config")).expect("config directory");
        fs::create_dir_all(root.join("environment/test")).expect("environment directory");
        fs::create_dir_all(home.join(".config")).expect("home config directory");
        fs::write(root.join("config/targets.dotfile"), targets).expect("targets");
        fs::write(root.join("environment/test/manifest"), manifest).expect("manifest");
        let context = Context::new(root.clone(), home.clone(), home.join(".config/dotfile"))
            .expect("context");
        Self {
            _temporary: temporary,
            root,
            home,
            context,
        }
    }

    fn directory(&self, relative: &str) {
        fs::create_dir_all(self.root.join(relative)).expect("repository directory");
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("parent directory");
        fs::write(path, content).expect("repository file");
    }

    fn sync(&self, cli: &SyncCli) -> Result<dotfile_cli::event::Summary, String> {
        let (decisions, _server) = decision::channel();
        engine::reconcile(&self.context, "test", cli, &decisions, &VecSink::default())
    }
}

fn cli() -> SyncCli {
    SyncCli {
        profile: Some("test".to_string()),
        dry_run: false,
        overrides: Vec::new(),
        force: false,
        resolve: Resolution::Skip,
        push: false,
        to: None,
        verbose: false,
    }
}

#[cfg(unix)]
#[test]
fn atomic_writes_preserve_modes_use_sane_defaults_and_skip_identical_content() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().unwrap();
    let existing = temporary.path().join("existing");
    fs::write(&existing, "before").unwrap();
    fs::set_permissions(&existing, fs::Permissions::from_mode(0o755)).unwrap();
    write_atomic(&existing, b"after").unwrap();
    assert_eq!(
        fs::metadata(&existing).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let modified = fs::metadata(&existing).unwrap().modified().unwrap();
    write_atomic(&existing, b"after").unwrap();
    assert_eq!(
        fs::metadata(&existing).unwrap().modified().unwrap(),
        modified
    );

    let created = temporary.path().join("created");
    write_atomic(&created, b"new").unwrap();
    assert_eq!(
        fs::metadata(created).unwrap().permissions().mode() & 0o777,
        0o644
    );

    let directory = temporary.path().join("not-a-file");
    fs::create_dir(&directory).unwrap();
    assert!(write_atomic(&directory, b"replacement").is_err());
    assert!(directory.is_dir());
}

#[test]
fn invalid_saved_override_aborts_before_any_link_mutation() {
    let sandbox = Sandbox::new("shared\n", "shared/git/.gitconfig = ~/.gitconfig\n");
    sandbox.write("shared/git/.gitconfig", "repo\n");
    fs::create_dir_all(&sandbox.context.state).unwrap();
    fs::write(&sandbox.context.overrides_file, [0xff]).unwrap();
    assert!(sandbox.sync(&cli()).is_err());
    assert!(!sandbox.home.join(".gitconfig").exists());
    assert_eq!(fs::read(&sandbox.context.overrides_file).unwrap(), [0xff]);
}

#[test]
fn invalid_link_index_aborts_instead_of_falling_back_to_a_false_clean_scan() {
    let sandbox = Sandbox::new("shared\n", "shared/git/.gitconfig = ~/.gitconfig\n");
    sandbox.write("shared/git/.gitconfig", "repo\n");
    fs::create_dir_all(&sandbox.context.state).unwrap();
    fs::write(sandbox.context.state.join("links"), [0xff]).unwrap();
    assert!(sandbox.sync(&cli()).is_err());
    assert!(!sandbox.home.join(".gitconfig").exists());
}

#[test]
fn dry_run_and_reconcile_share_a_deterministic_link_plan() {
    let sandbox = Sandbox::new("shared\n", "shared/git/.gitconfig = ~/.gitconfig\n");
    sandbox.write("shared/git/.gitconfig", "[user]\nname = Test\n");
    let mut options = cli();
    options.dry_run = true;
    let planned = sandbox.sync(&options).expect("dry-run plan");
    assert_eq!(planned.changed, 1);
    assert_eq!(planned.links, 1);
    assert!(!sandbox.home.join(".gitconfig").exists());
    assert!(!sandbox.context.state.join("profile").exists());

    options.dry_run = false;
    let applied = sandbox.sync(&options).expect("apply plan");
    assert_eq!(applied.changed, planned.changed);
    assert_eq!(
        fs::read_link(sandbox.home.join(".gitconfig")).expect("link target"),
        sandbox.root.join("shared/git/.gitconfig")
    );
    assert_eq!(
        fs::read_to_string(sandbox.context.state.join("profile")).unwrap(),
        "test\n"
    );

    let current = sandbox.sync(&options).expect("idempotent sync");
    assert_eq!(current.changed, 0);
    assert!(current.checked >= 1);
}

#[test]
fn unmanaged_conflict_is_reported_once_and_never_replaced() {
    let sandbox = Sandbox::new("shared\n", "shared/git/.gitconfig = ~/.gitconfig\n");
    sandbox.write("shared/git/.gitconfig", "repo\n");
    fs::write(sandbox.home.join(".gitconfig"), "live\n").expect("live file");
    let sink = VecSink::default();
    let (decisions, _server) = decision::channel();
    let result = engine::reconcile(&sandbox.context, "test", &cli(), &decisions, &sink);
    assert!(result.is_err());
    assert_eq!(
        fs::read_to_string(sandbox.home.join(".gitconfig")).unwrap(),
        "live\n"
    );
    let warnings = sink
        .events()
        .into_iter()
        .filter(|event| matches!(event, Event::Warning { .. }))
        .count();
    assert_eq!(warnings, 1);
}

#[test]
fn changing_override_prunes_the_old_layer_and_restores_the_base() {
    let sandbox = Sandbox::new("shared\n", "");
    sandbox.write("shared/zsh/config", "base\n");
    sandbox.write("shared/overrides/laptop/zsh/config", "laptop\n");
    let mut options = cli();
    options.overrides = vec!["shared=laptop".to_string()];
    sandbox.sync(&options).expect("laptop override");
    let destination = sandbox.home.join(".config/zsh/config");
    assert_eq!(
        fs::read_link(&destination).unwrap(),
        sandbox.root.join("shared/overrides/laptop/zsh/config")
    );

    options.overrides = vec!["shared=none".to_string()];
    sandbox.sync(&options).expect("base override");
    assert_eq!(
        fs::read_link(&destination).unwrap(),
        sandbox.root.join("shared/zsh/config")
    );
    assert_eq!(
        fs::read_to_string(&sandbox.context.overrides_file).unwrap(),
        "shared=none\n"
    );
}

#[cfg(unix)]
#[test]
fn missing_link_index_runs_one_time_bounded_migration() {
    let sandbox = Sandbox::new("shared\n", "shared/git/.gitconfig = ~/.gitconfig\n");
    sandbox.write("shared/git/.gitconfig", "repo\n");
    let stale = sandbox.home.join(".config/stale-link");
    std::os::unix::fs::symlink(sandbox.root.join("shared/removed"), &stale).unwrap();
    assert!(!sandbox.context.state.join("links").exists());
    sandbox.sync(&cli()).expect("migration sync");
    assert!(fs::symlink_metadata(stale).is_err());
    let index = fs::read_to_string(sandbox.context.state.join("links")).unwrap();
    assert!(index.contains(".gitconfig"));
}

#[cfg(unix)]
#[test]
fn warm_sync_uses_the_managed_link_index_instead_of_rescanning_home() {
    let sandbox = Sandbox::new("shared\n", "shared/git/.gitconfig = ~/.gitconfig\n");
    sandbox.write("shared/git/.gitconfig", "repo\n");
    sandbox.sync(&cli()).expect("indexed sync");
    let unindexed = sandbox.home.join(".config/unindexed-link");
    std::os::unix::fs::symlink(sandbox.root.join("shared/removed"), &unindexed).unwrap();
    sandbox.sync(&cli()).expect("warm indexed sync");
    assert!(
        fs::symlink_metadata(unindexed)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[cfg(unix)]
#[test]
fn migrated_folded_merge_package_rebuilds_only_eligible_children() {
    let sandbox = Sandbox::new(
        "shared\nmacos\n",
        "shared/vscode = ~/.config/Code/User\nmacos/vscode = ~/.config/Code/User\n",
    );
    sandbox.write("shared/vscode/settings.json", "{\"shared\": true}\n");
    sandbox.write("shared/vscode/keybindings.json", "[]\n");
    sandbox.write("shared/vscode/merge.dotfile", "ignore  machine.local\n");
    sandbox.write("macos/vscode/settings.macos.json", "{\"platform\": true}\n");
    let destination = sandbox.home.join(".config/Code/User");
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(sandbox.root.join("shared/vscode"), &destination).unwrap();
    let events = VecSink::default();
    let (decisions, _server) = decision::channel();
    let result = engine::reconcile(&sandbox.context, "test", &cli(), &decisions, &events);
    assert!(result.is_ok(), "{:#?}", events.events());
    assert!(destination.is_dir());
    assert!(
        !fs::symlink_metadata(&destination)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_link(destination.join("keybindings.json")).unwrap(),
        sandbox.root.join("shared/vscode/keybindings.json")
    );
    assert!(destination.join("settings.json").is_file());
    assert!(
        !fs::symlink_metadata(destination.join("settings.json"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!destination.join("merge.dotfile").exists());
    assert!(!destination.join("settings.macos.json").exists());
}

#[cfg(unix)]
#[test]
fn unfolding_a_previous_layer_preserves_its_eligible_children() {
    let sandbox = Sandbox::new("shared\nmacos\n", "");
    sandbox.write("shared/tool/base.conf", "base\n");
    sandbox.write("macos/tool/platform.conf", "platform\n");
    let destination = sandbox.home.join(".config/tool");
    std::os::unix::fs::symlink(sandbox.root.join("shared/tool"), &destination).unwrap();
    sandbox.sync(&cli()).expect("expand layered package");
    assert!(destination.is_dir());
    assert_eq!(
        fs::read_link(destination.join("base.conf")).unwrap(),
        sandbox.root.join("shared/tool/base.conf")
    );
    assert_eq!(
        fs::read_link(destination.join("platform.conf")).unwrap(),
        sandbox.root.join("macos/tool/platform.conf")
    );
}

#[cfg(unix)]
#[test]
fn vault_descendants_force_filtered_directory_expansion() {
    let sandbox = Sandbox::new("shared\n", "");
    sandbox.write("shared/app/config.toml", "enabled = true\n");
    sandbox.write("shared/app/private/token.enc", "sealed\n");
    sandbox.write("shared/app/private/render.tmpl", "rendered\n");
    sandbox
        .sync(&cli())
        .expect("expand package containing vault files");
    let destination = sandbox.home.join(".config/app");
    assert!(destination.is_dir());
    assert!(
        !fs::symlink_metadata(&destination)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_link(destination.join("config.toml")).unwrap(),
        sandbox.root.join("shared/app/config.toml")
    );
    assert!(!destination.join("private/token.enc").exists());
    assert!(!destination.join("private/render.tmpl").exists());
    assert_eq!(
        fs::read_to_string(destination.join("private/render")).unwrap(),
        "rendered\n"
    );
    assert!(
        !fs::symlink_metadata(destination.join("private/render"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn overlays_materialize_and_live_adoption_preserves_jsonc_comments() {
    let sandbox = Sandbox::new(
        "shared\nmacos\n",
        "shared/vscode/settings.json = ~/.config/Code/User/settings.json\nmacos/vscode = ~/.config/Code/User\n",
    );
    sandbox.write(
        "shared/vscode/settings.json",
        "{\n    // shared marker\n    \"git.autofetch\": true,\n}\n",
    );
    sandbox.write(
        "macos/vscode/settings.macos.json",
        "{\n    // overlay marker\n    \"shellformat.path\": \"/opt/homebrew/bin/shfmt\"\n}\n",
    );
    sandbox.sync(&cli()).expect("initial materialization");
    let destination = sandbox.home.join(".config/Code/User/settings.json");
    let document: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&destination).unwrap()).unwrap();
    assert_eq!(document["git.autofetch"], true);
    assert_eq!(document["shellformat.path"], "/opt/homebrew/bin/shfmt");

    fs::write(
        &destination,
        "{\n  \"git.autofetch\": true,\n  \"shellformat.path\": \"/usr/bin/shfmt\",\n  \"editor.fontSize\": 14\n}\n",
    )
    .expect("live edit");
    let mut adopt = cli();
    adopt.resolve = Resolution::Live;
    sandbox.sync(&adopt).expect("adopt live changes");
    let shared = fs::read_to_string(sandbox.root.join("shared/vscode/settings.json")).unwrap();
    let overlay =
        fs::read_to_string(sandbox.root.join("macos/vscode/settings.macos.json")).unwrap();
    assert!(shared.contains("// shared marker"));
    assert!(shared.contains("\"editor.fontSize\": 14"));
    assert!(overlay.contains("// overlay marker"));
    assert!(overlay.contains("\"shellformat.path\": \"/usr/bin/shfmt\""));
    assert!(!overlay.contains("\"editor.fontSize\": 14"));
    let mut state_files = vec![
        sandbox.context.state.join("profile"),
        sandbox.context.state.join("overrides"),
        sandbox.context.state.join("links"),
    ];
    state_files.extend(
        fs::read_dir(sandbox.context.state.join("merge"))
            .unwrap()
            .flatten()
            .map(|entry| entry.path()),
    );
    let modified = state_files
        .iter()
        .map(|path| {
            (
                path.clone(),
                fs::metadata(path).unwrap().modified().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(sandbox.sync(&cli()).expect("settled sync").changed, 0);
    for (path, before) in modified {
        assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), before);
    }
}

#[test]
fn unresolved_merge_exposes_key_details_without_rewriting_live_state() {
    let sandbox = Sandbox::new(
        "shared\nmacos\n",
        "shared/vscode/settings.json = ~/.config/Code/User/settings.json\nmacos/vscode = ~/.config/Code/User\n",
    );
    sandbox.write("shared/vscode/settings.json", "{\"a\": 1}\n");
    sandbox.write("macos/vscode/settings.macos.json", "{\"b\": 2}\n");
    sandbox.sync(&cli()).expect("initial materialization");
    let destination = sandbox.home.join(".config/Code/User/settings.json");
    fs::write(&destination, "{\"a\": 9, \"b\": 2}\n").unwrap();
    let sink = VecSink::default();
    let (decisions, server) = decision::channel();
    let responder = std::thread::spawn(move || {
        loop {
            if let Some(request) = server.try_recv() {
                server
                    .respond(&request, Choice::Skip)
                    .expect("skip response");
                break;
            }
            std::thread::yield_now();
        }
    });
    assert!(engine::reconcile(&sandbox.context, "test", &cli(), &decisions, &sink).is_err());
    responder.join().expect("decision responder");
    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "{\"a\": 9, \"b\": 2}\n"
    );
    assert!(sink.events().iter().any(|event| matches!(
        event,
        Event::Item { action: Action::Merge, detail, .. }
            if detail.contains("modify:a") && detail.contains("1") && detail.contains("9")
    )));
}

#[test]
fn invalid_merge_rules_abort_before_materializing_the_destination() {
    let sandbox = Sandbox::new(
        "shared\nmacos\n",
        "shared/vscode/settings.json = ~/.config/Code/User/settings.json\n",
    );
    sandbox.write("shared/vscode/settings.json", "{\"a\": 1}\n");
    sandbox.write("macos/vscode/settings.macos.json", "{\"b\": 2}\n");
    fs::write(sandbox.root.join("shared/vscode/merge.dotfile"), [0xff]).unwrap();
    assert!(sandbox.sync(&cli()).is_err());
    assert!(
        !sandbox
            .home
            .join(".config/Code/User/settings.json")
            .exists()
    );
}

#[test]
fn interactive_key_choices_are_collected_before_mixed_resolution_mutates_files() {
    let sandbox = Sandbox::new(
        "shared\nmacos\n",
        "shared/vscode/settings.json = ~/.config/Code/User/settings.json\nmacos/vscode = ~/.config/Code/User\n",
    );
    sandbox.write(
        "shared/vscode/settings.json",
        "{\n    // keep base\n    \"a\": 1\n}\n",
    );
    sandbox.write(
        "macos/vscode/settings.macos.json",
        "{\n    // keep overlay\n    \"b\": 2\n}\n",
    );
    sandbox.sync(&cli()).expect("initial materialization");
    let destination = sandbox.home.join(".config/Code/User/settings.json");
    fs::write(&destination, "{\"a\": 9, \"b\": 8}\n").unwrap();
    let overlay = sandbox.root.join("macos/vscode/settings.macos.json");
    let before_overlay = fs::read_to_string(&overlay).unwrap();
    let observed_destination = destination.clone();
    let observed_overlay = overlay.clone();
    let (decisions, server) = decision::channel();
    let responder = std::thread::spawn(move || {
        let mut seen = 0;
        loop {
            if let Some(request) = server.try_recv() {
                assert_eq!(
                    fs::read_to_string(&observed_destination).unwrap(),
                    "{\"a\": 9, \"b\": 8}\n"
                );
                assert_eq!(
                    fs::read_to_string(&observed_overlay).unwrap(),
                    before_overlay
                );
                let choice = match (&request.prompt, seen) {
                    (Prompt::Merge { .. }, 0) => Choice::Repo,
                    (Prompt::Merge { .. }, 1) => Choice::Live,
                    (Prompt::MergeTarget { default, .. }, 2) => Choice::Target(*default),
                    _ => panic!("unexpected decision prompt: {:?}", request.prompt),
                };
                server.respond(&request, choice).expect("decision response");
                seen += 1;
                if seen == 3 {
                    break;
                }
            }
            std::thread::yield_now();
        }
    });
    engine::reconcile(
        &sandbox.context,
        "test",
        &cli(),
        &decisions,
        &VecSink::default(),
    )
    .expect("mixed resolution");
    responder.join().expect("decision responder");
    let live: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&destination).unwrap()).unwrap();
    assert_eq!(live["a"], 1);
    assert_eq!(live["b"], 8);
    let overlay = fs::read_to_string(&overlay).unwrap();
    assert!(overlay.contains("// keep overlay"));
    assert!(overlay.contains("\"b\": 8"));
}

#[test]
fn interactive_live_addition_defaults_to_the_active_overlay() {
    let sandbox = Sandbox::new(
        "shared\nmacos\n",
        "shared/vscode/settings.json = ~/.config/Code/User/settings.json\nmacos/vscode = ~/.config/Code/User\n",
    );
    sandbox.write("shared/vscode/settings.json", "{\"a\": 1}\n");
    sandbox.write("macos/vscode/settings.macos.json", "{\"b\": 2}\n");
    sandbox.sync(&cli()).expect("initial materialization");
    let destination = sandbox.home.join(".config/Code/User/settings.json");
    fs::write(
        &destination,
        "{\"a\": 1, \"b\": 2, \"platform.only\": true}\n",
    )
    .unwrap();
    let (decisions, server) = decision::channel();
    let responder = std::thread::spawn(move || {
        let first = loop {
            if let Some(request) = server.try_recv() {
                break request;
            }
            std::thread::yield_now();
        };
        server.respond(&first, Choice::Live).unwrap();
        let target = loop {
            if let Some(request) = server.try_recv() {
                break request;
            }
            std::thread::yield_now();
        };
        let Prompt::MergeTarget {
            targets, default, ..
        } = &target.prompt
        else {
            panic!("expected target prompt")
        };
        assert_eq!(targets, &["shared", "macos"]);
        assert_eq!(*default, 1);
        server.respond(&target, Choice::Target(*default)).unwrap();
    });
    engine::reconcile(
        &sandbox.context,
        "test",
        &cli(),
        &decisions,
        &VecSink::default(),
    )
    .expect("interactive adoption");
    responder.join().unwrap();
    let shared = fs::read_to_string(sandbox.root.join("shared/vscode/settings.json")).unwrap();
    let overlay =
        fs::read_to_string(sandbox.root.join("macos/vscode/settings.macos.json")).unwrap();
    assert!(!shared.contains("platform.only"));
    assert!(overlay.contains("platform.only"));
}

#[test]
fn interactive_ignore_preserves_live_value_without_polluting_a_layer() {
    let sandbox = Sandbox::new(
        "shared\nmacos\n",
        "shared/vscode/settings.json = ~/.config/Code/User/settings.json\nmacos/vscode = ~/.config/Code/User\n",
    );
    sandbox.write("shared/vscode/settings.json", "{\"a\": 1}\n");
    sandbox.write("macos/vscode/settings.macos.json", "{\"b\": 2}\n");
    sandbox.sync(&cli()).expect("initial materialization");
    let destination = sandbox.home.join(".config/Code/User/settings.json");
    fs::write(
        &destination,
        "{\"a\": 1, \"b\": 2, \"machine.local\": true}\n",
    )
    .unwrap();
    let (decisions, server) = decision::channel();
    let responder = std::thread::spawn(move || {
        loop {
            if let Some(request) = server.try_recv() {
                server.respond(&request, Choice::Ignore).unwrap();
                break;
            }
            std::thread::yield_now();
        }
    });
    engine::reconcile(
        &sandbox.context,
        "test",
        &cli(),
        &decisions,
        &VecSink::default(),
    )
    .expect("ignore local value");
    responder.join().unwrap();
    let rules = fs::read_to_string(sandbox.root.join("shared/vscode/merge.dotfile")).unwrap();
    assert!(rules.contains("ignore  machine.local"));
    assert!(
        !fs::read_to_string(sandbox.root.join("shared/vscode/settings.json"))
            .unwrap()
            .contains("machine.local")
    );
    assert!(
        !fs::read_to_string(sandbox.root.join("macos/vscode/settings.macos.json"))
            .unwrap()
            .contains("machine.local")
    );
    assert_eq!(
        sandbox
            .sync(&cli())
            .expect("ignored value is settled")
            .changed,
        0
    );
}

#[test]
fn interactive_live_addition_defaults_to_the_last_materialized_host_overlay() {
    let sandbox = Sandbox::new(
        "shared\nlinux/common\nlinux/arch\nlinux/hyprland\n",
        "shared/vscode/settings.json = ~/.config/Code/User/settings.json\n",
    );
    sandbox.write("shared/vscode/settings.json", "{\"base\": true}\n");
    sandbox.write(
        "linux/common/vscode/settings.common.json",
        "{\"common\": true}\n",
    );
    sandbox.write("linux/arch/vscode/settings.arch.json", "{\"arch\": true}\n");
    sandbox.sync(&cli()).expect("initial materialization");
    let destination = sandbox.home.join(".config/Code/User/settings.json");
    fs::write(
        &destination,
        "{\"base\":true,\"common\":true,\"arch\":true,\"host.only\":true}\n",
    )
    .unwrap();
    let (decisions, server) = decision::channel();
    let responder = std::thread::spawn(move || {
        let first = loop {
            if let Some(request) = server.try_recv() {
                break request;
            }
            std::thread::yield_now();
        };
        server.respond(&first, Choice::Live).unwrap();
        let target = loop {
            if let Some(request) = server.try_recv() {
                break request;
            }
            std::thread::yield_now();
        };
        let Prompt::MergeTarget {
            targets, default, ..
        } = &target.prompt
        else {
            panic!("expected target prompt")
        };
        assert_eq!(targets, &["shared", "common", "arch", "hyprland"]);
        assert_eq!(*default, 2);
        server.respond(&target, Choice::Target(*default)).unwrap();
    });
    engine::reconcile(
        &sandbox.context,
        "test",
        &cli(),
        &decisions,
        &VecSink::default(),
    )
    .expect("interactive adoption");
    responder.join().unwrap();
    assert!(
        fs::read_to_string(sandbox.root.join("linux/arch/vscode/settings.arch.json"))
            .unwrap()
            .contains("host.only")
    );
    assert!(
        !sandbox
            .root
            .join("linux/hyprland/vscode/settings.hyprland.json")
            .exists()
    );
}

#[test]
fn invalid_late_ignore_decision_leaves_every_merged_file_untouched() {
    let sandbox = Sandbox::new(
        "shared\nmacos\n",
        "shared/one/settings.json = ~/.config/a.json\nshared/two/settings.json = ~/.config/b.json\n",
    );
    sandbox.write("shared/one/settings.json", "{\"a\": 1}\n");
    sandbox.write("macos/one/settings.macos.json", "{\"platform\": 1}\n");
    sandbox.write("shared/two/settings.json", "{\"b\": 2}\n");
    sandbox.write("macos/two/settings.macos.json", "{\"platform\": 2}\n");
    sandbox.sync(&cli()).expect("initial materialization");
    let first = sandbox.home.join(".config/a.json");
    let second = sandbox.home.join(".config/b.json");
    fs::write(&first, "{\"a\": 9, \"platform\": 1}\n").unwrap();
    fs::write(&second, "{\"b\": 2, \"platform\": 2, \"bad/key\": true}\n").unwrap();
    let before_first = fs::read(&first).unwrap();
    let before_second = fs::read(&second).unwrap();
    let (decisions, server) = decision::channel();
    let responder = std::thread::spawn(move || {
        let mut seen = 0;
        while seen < 2 {
            if let Some(request) = server.try_recv() {
                let choice = if seen == 0 {
                    Choice::Repo
                } else {
                    Choice::Ignore
                };
                server.respond(&request, choice).unwrap();
                seen += 1;
            }
            std::thread::yield_now();
        }
    });
    let result = engine::reconcile(
        &sandbox.context,
        "test",
        &cli(),
        &decisions,
        &VecSink::default(),
    );
    responder.join().unwrap();
    assert!(result.is_err());
    assert_eq!(fs::read(&first).unwrap(), before_first);
    assert_eq!(fs::read(&second).unwrap(), before_second);
    assert!(!sandbox.root.join("shared/two/merge.dotfile").exists());
}

#[cfg(unix)]
#[test]
fn secret_templates_materialize_privately_and_are_idempotent() {
    use std::os::unix::fs::PermissionsExt;

    let sandbox = Sandbox::new("shared\n", "");
    sandbox.directory("shared/credentials");
    sandbox.write("shared/credentials/.secret", "");
    sandbox.write("shared/credentials/token.tmpl", "literal-token\n");
    let first = sandbox.sync(&cli()).expect("materialize secret");
    assert_eq!(first.secrets, 1);
    let destination = sandbox.home.join(".config/credentials/token");
    assert_eq!(fs::read_to_string(&destination).unwrap(), "literal-token\n");
    assert_eq!(
        fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(sandbox.sync(&cli()).expect("current secret").changed, 0);
}

#[test]
fn blocked_plaintext_secret_fails_the_sync_before_success_state_is_saved() {
    let sandbox = Sandbox::new("shared\n", "");
    sandbox.directory("shared/credentials");
    sandbox.write("shared/credentials/.secret", "");
    sandbox.write("shared/credentials/password", "plaintext\n");
    let result = sandbox.sync(&cli());
    assert!(result.is_err());
    assert!(!sandbox.home.join(".config/credentials/password").exists());
    assert!(!sandbox.context.state.join("links").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_secret_path_aborts_before_secret_application() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let sandbox = Sandbox::new("shared\n", "");
    sandbox.directory("shared/credentials");
    sandbox.write("shared/credentials/.secret", "");
    let path = sandbox
        .root
        .join("shared/credentials")
        .join(OsString::from_vec(b"token-\xff".to_vec()));
    fs::write(path, "plaintext\n").unwrap();
    assert!(sandbox.sync(&cli()).is_err());
    assert!(!sandbox.home.join(".config/credentials").exists());
}

#[test]
fn hyprland_integration_expands_home_and_prunes_the_broken_local_override() {
    let sandbox = Sandbox::new("linux/hyprland\n", "");
    sandbox.write(
        "linux/hyprland/elephant/files.toml",
        "search_dirs = [\"$HOME/Documents\"]\n",
    );
    sandbox.directory("linux/hyprland/hypr/conf.d");
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        sandbox.home.join("missing-local.conf"),
        sandbox.root.join("linux/hyprland/hypr/conf.d/local.conf"),
    )
    .expect("broken local override");
    let summary = sandbox.sync(&cli()).expect("hyprland integration");
    let generated = sandbox.home.join(".config/elephant/files.toml");
    assert_eq!(
        fs::read_to_string(generated).unwrap(),
        format!("search_dirs = [\"{}/Documents\"]\n", sandbox.home.display())
    );
    #[cfg(unix)]
    assert!(
        fs::symlink_metadata(sandbox.root.join("linux/hyprland/hypr/conf.d/local.conf")).is_err()
    );
    assert!(summary.generated >= 1);
}

#[test]
fn package_metadata_generation_is_stable_and_dry_run_is_read_only() {
    let sandbox = Sandbox::new("shared\ncustom\n", "");
    sandbox.directory("shared/alpha");
    sandbox.directory("shared/long-package");
    sandbox.directory("custom/tool");
    sandbox.write(
        "config/packages.dotfile",
        "shared {\n  alpha  = First package\n  long-package\n}\n\ncustom {\n  tool  = Custom tool\n}",
    );
    fs::write(&sandbox.context.packages_doc, "stale\n").expect("stale package document");
    let sink = VecSink::default();
    assert_eq!(
        packages::synchronize(&sandbox.context, true, &sink).unwrap(),
        1
    );
    assert_eq!(
        fs::read_to_string(&sandbox.context.packages_doc).unwrap(),
        "stale\n"
    );
    assert_eq!(
        packages::synchronize(&sandbox.context, false, &sink).unwrap(),
        1
    );
    let config = fs::read_to_string(&sandbox.context.packages_config).unwrap();
    let document = fs::read_to_string(&sandbox.context.packages_doc).unwrap();
    assert!(config.contains("alpha  = First package"));
    assert!(config.contains("tool  = Custom tool"));
    assert!(document.contains("- `alpha` — First package"));
    assert_eq!(
        packages::synchronize(&sandbox.context, false, &sink).unwrap(),
        0
    );
}

#[test]
fn invalid_package_metadata_never_rewrites_generated_artifacts() {
    let sandbox = Sandbox::new("shared\n", "");
    sandbox.directory("shared/tool");
    fs::write(&sandbox.context.packages_config, [0xff]).unwrap();
    fs::write(&sandbox.context.packages_doc, "preserve me\n").unwrap();
    let result = packages::synchronize(&sandbox.context, false, &VecSink::default());
    assert!(result.is_err());
    assert_eq!(fs::read(&sandbox.context.packages_config).unwrap(), [0xff]);
    assert_eq!(
        fs::read_to_string(&sandbox.context.packages_doc).unwrap(),
        "preserve me\n"
    );
}

#[test]
fn invalid_manifest_never_rewrites_generated_package_artifacts() {
    let sandbox = Sandbox::new("shared\n", "");
    sandbox.directory("shared/tool");
    fs::write(sandbox.root.join("environment/test/manifest"), [0xff]).unwrap();
    fs::write(
        &sandbox.context.packages_config,
        "shared {\n  tool  = preserve config\n}",
    )
    .unwrap();
    fs::write(&sandbox.context.packages_doc, "preserve docs\n").unwrap();
    let result = packages::synchronize(&sandbox.context, false, &VecSink::default());
    assert!(result.is_err());
    assert_eq!(
        fs::read_to_string(&sandbox.context.packages_config).unwrap(),
        "shared {\n  tool  = preserve config\n}"
    );
    assert_eq!(
        fs::read_to_string(&sandbox.context.packages_doc).unwrap(),
        "preserve docs\n"
    );
}

#[test]
fn checked_in_package_artifacts_match_the_native_renderer() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("repository root")
        .to_path_buf();
    let temporary = tempfile::tempdir().expect("temporary home");
    let context = Context::new(
        root,
        temporary.path().to_path_buf(),
        temporary.path().join(".config/dotfile"),
    )
    .expect("repository context");
    let sink = VecSink::default();
    let changed = packages::synchronize(&context, true, &sink).unwrap();
    assert_eq!(changed, 0, "{:?}", sink.events());
}
