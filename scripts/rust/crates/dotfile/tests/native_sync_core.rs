#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;

use dotfile_cli::artifacts::packages;
use dotfile_cli::cli::{Resolution, SyncCli};
use dotfile_cli::context::{Context, write_atomic};
use dotfile_cli::decision::{self, Choice, Prompt};
use dotfile_cli::event::{Action, Event, VecSink};
use dotfile_cli::sync::engine;
use testkit::{Bin, Ran, TempDir, tree_pairs};

struct Sandbox {
    _temporary: TempDir,
    root: PathBuf,
    home: PathBuf,
    context: Context,
}

impl Sandbox {
    fn new(manifest: &str, targets: &str) -> Self {
        let temporary = tree_pairs(&[
            ("repo/config/targets.dotfile", targets),
            ("repo/environment/test/manifest", manifest),
            ("home/.config/", ""),
        ]);
        let root = temporary.path().join("repo");
        let home = temporary.path().join("home");
        let context = Context::new(
            root.clone(),
            home.clone(),
            root.join("config"),
            home.join(".config"),
        )
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
        self.sync_answering(cli, None, &VecSink::default())
    }

    fn sync_answering(
        &self,
        cli: &SyncCli,
        choice: Option<Choice>,
        sink: &VecSink,
    ) -> Result<dotfile_cli::event::Summary, String> {
        let (decisions, server) = decision::channel();
        let responder = std::thread::spawn(move || {
            while let Some(request) = server.next() {
                let answer = choice.unwrap_or_else(|| request.prompt.safe_default());
                if server.respond(&request, answer).is_err() {
                    break;
                }
            }
        });
        let outcome = engine::reconcile(&self.context, "test", cli, &decisions, sink);
        drop(decisions);
        let _ = responder.join();
        outcome
    }

    fn package_docs(&self, args: &[&str]) -> Ran {
        Bin::new(env!("CARGO_BIN_EXE_dotfile"))
            .args(["docs", "--only", "packages", "--json"])
            .args(args)
            .env("DOTFILE_ROOT", &self.root)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .run()
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

    let temporary = TempDir::new().unwrap();
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
    fs::create_dir_all(&sandbox.context.root_config).unwrap();
    fs::write(&sandbox.context.overrides_file, [0xff]).unwrap();
    assert!(sandbox.sync(&cli()).is_err());
    assert!(!sandbox.home.join(".gitconfig").exists());
    assert_eq!(fs::read(&sandbox.context.overrides_file).unwrap(), [0xff]);
}

#[test]
fn invalid_link_index_aborts_instead_of_falling_back_to_a_false_clean_scan() {
    let sandbox = Sandbox::new("shared\n", "shared/git/.gitconfig = ~/.gitconfig\n");
    sandbox.write("shared/git/.gitconfig", "repo\n");
    fs::create_dir_all(&sandbox.context.root_config).unwrap();
    fs::write(sandbox.context.root_config.join("links"), [0xff]).unwrap();
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
    assert!(!sandbox.context.root_config.join("profile").exists());

    options.dry_run = false;
    let applied = sandbox.sync(&options).expect("apply plan");
    assert_eq!(applied.changed, planned.changed);
    assert_eq!(
        fs::read_link(sandbox.home.join(".gitconfig")).expect("link target"),
        sandbox.root.join("shared/git/.gitconfig")
    );
    assert_eq!(
        fs::read_to_string(sandbox.context.root_config.join("profile")).unwrap(),
        "test\n"
    );

    let current = sandbox.sync(&options).expect("idempotent sync");
    assert_eq!(current.changed, 0);
    assert!(current.checked >= 1);
}

#[cfg(unix)]
#[test]
fn settled_layered_targets_keep_the_final_symlinks_untouched() {
    use std::os::unix::fs::{MetadataExt, symlink};

    let sandbox = Sandbox::new(
        "shared\nmacos\n",
        "shared/git/.gitconfig = ~/.gitconfig\nmacos/git/.gitconfig = ~/.gitconfig\n\
         shared/fastfetch = ~/.config/fastfetch\nmacos/fastfetch = ~/.config/fastfetch\n",
    );
    sandbox.write("shared/git/.gitconfig", "shared\n");
    sandbox.write("macos/git/.gitconfig", "macos\n");
    sandbox.write("shared/fastfetch/config.jsonc", "shared\n");
    sandbox.write("macos/fastfetch/config.jsonc", "macos\n");
    let git = sandbox.home.join(".gitconfig");
    let fastfetch_directory = sandbox.home.join(".config/fastfetch");
    let fastfetch = fastfetch_directory.join("config.jsonc");
    fs::create_dir_all(&fastfetch_directory).unwrap();
    symlink(sandbox.root.join("macos/git/.gitconfig"), &git).unwrap();
    symlink(
        sandbox.root.join("macos/fastfetch/config.jsonc"),
        &fastfetch,
    )
    .unwrap();
    let identity = |path: &std::path::Path| {
        let metadata = fs::symlink_metadata(path).unwrap();
        (
            metadata.ino(),
            metadata.modified().unwrap(),
            fs::read_link(path).unwrap(),
        )
    };
    let before = (identity(&git), identity(&fastfetch));
    let first = sandbox.sync(&cli()).expect("first settled sync");
    let after_first = (identity(&git), identity(&fastfetch));
    let second = sandbox.sync(&cli()).expect("second settled sync");
    let after_second = (identity(&git), identity(&fastfetch));
    assert_eq!(first.changed, 0);
    assert_eq!(second.changed, 0);
    assert_eq!(after_first, before);
    assert_eq!(after_second, before);
}

#[cfg(unix)]
#[test]
fn fresh_layered_targets_count_each_final_destination_once() {
    let sandbox = Sandbox::new(
        "shared\nmacos\n",
        "shared/git/.gitconfig = ~/.gitconfig\nmacos/git/.gitconfig = ~/.gitconfig\n\
         shared/fastfetch = ~/.config/fastfetch\nmacos/fastfetch = ~/.config/fastfetch\n",
    );
    sandbox.write("shared/git/.gitconfig", "shared\n");
    sandbox.write("macos/git/.gitconfig", "macos\n");
    sandbox.write("shared/fastfetch/config.jsonc", "shared\n");
    sandbox.write("macos/fastfetch/config.jsonc", "macos\n");
    let summary = sandbox.sync(&cli()).expect("layered sync");
    assert_eq!(summary.changed, 2);
    assert_eq!(
        fs::read_link(sandbox.home.join(".gitconfig")).unwrap(),
        sandbox.root.join("macos/git/.gitconfig")
    );
    assert_eq!(
        fs::read_link(sandbox.home.join(".config/fastfetch/config.jsonc")).unwrap(),
        sandbox.root.join("macos/fastfetch/config.jsonc")
    );
    assert_eq!(sandbox.sync(&cli()).unwrap().changed, 0);
}

#[cfg(unix)]
#[test]
fn later_directory_replaces_an_earlier_file_at_the_same_destination() {
    let sandbox = Sandbox::new(
        "shared\nmacos\n",
        "shared/lower/item = ~/.config/final\nmacos/upper/item = ~/.config/final\n",
    );
    sandbox.write("shared/lower/item", "lower\n");
    sandbox.write("macos/upper/item/child.conf", "higher\n");
    let destination = sandbox.home.join(".config/final");
    let mut options = cli();
    options.dry_run = true;
    assert_eq!(sandbox.sync(&options).expect("directory plan").changed, 1);
    assert!(!destination.exists());
    options.dry_run = false;
    assert_eq!(sandbox.sync(&options).expect("directory apply").changed, 1);
    assert_eq!(
        fs::read_link(&destination).unwrap(),
        sandbox.root.join("macos/upper/item")
    );
    assert_eq!(sandbox.sync(&options).expect("directory warm").changed, 0);
}

#[cfg(unix)]
#[test]
fn later_file_replaces_an_earlier_directory_at_the_same_destination() {
    let sandbox = Sandbox::new(
        "shared\nmacos\n",
        "shared/lower/item = ~/.config/final\nmacos/upper/item = ~/.config/final\n",
    );
    sandbox.write("shared/lower/item/child.conf", "lower\n");
    sandbox.write("macos/upper/item", "higher\n");
    let destination = sandbox.home.join(".config/final");
    let mut options = cli();
    options.dry_run = true;
    assert_eq!(sandbox.sync(&options).expect("file plan").changed, 1);
    assert!(!destination.exists());
    options.dry_run = false;
    assert_eq!(sandbox.sync(&options).expect("file apply").changed, 1);
    assert_eq!(
        fs::read_link(&destination).unwrap(),
        sandbox.root.join("macos/upper/item")
    );
    assert_eq!(sandbox.sync(&options).expect("file warm").changed, 0);
}

#[cfg(unix)]
#[test]
fn stale_folded_directory_rebuilds_only_the_active_remapped_union() {
    use std::os::unix::fs::symlink;

    let sandbox = Sandbox::new(
        "shared\n",
        "shared/tool = ~/.config/tool\nshared/tool/moved.conf = ~/.config/moved.conf\n",
    );
    sandbox.write("shared/tool/moved.conf", "active\n");
    sandbox.write("inactive/tool/moved.conf", "inactive\n");
    sandbox.write("inactive/tool/old-only.conf", "stale\n");
    let destination = sandbox.home.join(".config/tool");
    symlink(sandbox.root.join("inactive/tool"), &destination).unwrap();
    assert_eq!(sandbox.sync(&cli()).expect("active rebuild").changed, 2);
    assert!(destination.is_dir());
    assert!(!destination.join("moved.conf").exists());
    assert!(!destination.join("old-only.conf").exists());
    assert_eq!(
        fs::read_link(sandbox.home.join(".config/moved.conf")).unwrap(),
        sandbox.root.join("shared/tool/moved.conf")
    );
    assert_eq!(sandbox.sync(&cli()).expect("settled rebuild").changed, 0);
}

#[cfg(unix)]
#[test]
fn later_file_replaces_only_a_fully_managed_expanded_directory() {
    use std::os::unix::fs::symlink;

    let sandbox = Sandbox::new(
        "shared\nmacos\n",
        "shared/lower/item = ~/.config/final\nmacos/upper/item = ~/.config/final\n",
    );
    sandbox.write("shared/lower/item/child.conf", "lower\n");
    sandbox.write("macos/upper/item", "higher\n");
    let destination = sandbox.home.join(".config/final");
    let child = destination.join("child.conf");
    fs::create_dir_all(&destination).unwrap();
    symlink(sandbox.root.join("shared/lower/item/child.conf"), &child).unwrap();
    fs::create_dir_all(&sandbox.context.root_config).unwrap();
    fs::write(
        sandbox.context.root_config.join("links"),
        format!("{}\n", child.display()),
    )
    .unwrap();
    assert_eq!(
        sandbox.sync(&cli()).expect("managed replacement").changed,
        1
    );
    assert_eq!(
        fs::read_link(&destination).unwrap(),
        sandbox.root.join("macos/upper/item")
    );
    assert_eq!(
        sandbox.sync(&cli()).expect("settled replacement").changed,
        0
    );
}

#[cfg(unix)]
#[test]
fn later_file_never_removes_an_unmanaged_directory() {
    let sandbox = Sandbox::new(
        "shared\nmacos\n",
        "shared/lower/item = ~/.config/final\nmacos/upper/item = ~/.config/final\n",
    );
    sandbox.write("shared/lower/item/child.conf", "lower\n");
    sandbox.write("macos/upper/item", "higher\n");
    let destination = sandbox.home.join(".config/final");
    fs::create_dir_all(&destination).unwrap();
    fs::create_dir(destination.join("empty")).unwrap();
    fs::write(destination.join("mine.conf"), "mine\n").unwrap();
    assert!(sandbox.sync(&cli()).is_err());
    assert!(destination.join("empty").is_dir());
    assert_eq!(
        fs::read_to_string(destination.join("mine.conf")).unwrap(),
        "mine\n"
    );
}

#[test]
fn declined_overwrite_reports_once_and_never_replaces() {
    let sandbox = Sandbox::new("shared\n", "shared/git/.gitconfig = ~/.gitconfig\n");
    sandbox.write("shared/git/.gitconfig", "repo\n");
    fs::write(sandbox.home.join(".gitconfig"), "live\n").expect("live file");
    let sink = VecSink::default();
    let result = sandbox.sync_answering(&cli(), Some(Choice::Keep), &sink);
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

#[cfg(unix)]
#[test]
fn approved_overwrite_replaces_an_unmanaged_file_with_its_link() {
    let sandbox = Sandbox::new("shared\n", "shared/git/.gitconfig = ~/.gitconfig\n");
    sandbox.write("shared/git/.gitconfig", "repo\n");
    let destination = sandbox.home.join(".gitconfig");
    fs::write(&destination, "live\n").expect("live file");
    let summary = sandbox
        .sync_answering(&cli(), Some(Choice::Overwrite), &VecSink::default())
        .expect("overwritten conflict");
    assert_eq!(summary.links, 1);
    assert_eq!(
        fs::read_link(&destination).unwrap(),
        sandbox.root.join("shared/git/.gitconfig")
    );
    assert_eq!(fs::read_to_string(&destination).unwrap(), "repo\n");
}

#[cfg(unix)]
#[test]
fn one_answer_for_all_settles_every_remaining_conflict() {
    let sandbox = Sandbox::new(
        "shared\n",
        "shared/git/.gitconfig = ~/.gitconfig\nshared/tmux/.tmux.conf = ~/.tmux.conf\n",
    );
    sandbox.write("shared/git/.gitconfig", "repo\n");
    sandbox.write("shared/tmux/.tmux.conf", "repo\n");
    fs::write(sandbox.home.join(".gitconfig"), "live\n").unwrap();
    fs::write(sandbox.home.join(".tmux.conf"), "live\n").unwrap();
    let (decisions, server) = decision::channel();
    let asked = std::thread::spawn(move || {
        let mut asked = 0;
        while let Some(request) = server.next() {
            asked += 1;
            if server.respond(&request, Choice::OverwriteAll).is_err() {
                break;
            }
        }
        asked
    });
    let summary = engine::reconcile(
        &sandbox.context,
        "test",
        &cli(),
        &decisions,
        &VecSink::default(),
    )
    .expect("batch overwrite");
    drop(decisions);
    assert_eq!(asked.join().expect("responder"), 1);
    assert_eq!(summary.links, 2);
    for name in [".gitconfig", ".tmux.conf"] {
        assert!(
            fs::symlink_metadata(sandbox.home.join(name))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}

#[cfg(unix)]
#[test]
fn approved_overwrite_discards_an_unmanaged_directory_and_its_contents() {
    let sandbox = Sandbox::new("shared\n", "shared/tmux/.tmux.conf = ~/.tmux.conf\n");
    sandbox.write("shared/tmux/.tmux.conf", "repo\n");
    let destination = sandbox.home.join(".tmux.conf");
    fs::create_dir_all(destination.join("nested")).unwrap();
    fs::write(destination.join("nested/mine.conf"), "mine\n").unwrap();
    sandbox
        .sync_answering(&cli(), Some(Choice::Overwrite), &VecSink::default())
        .expect("overwritten directory");
    assert_eq!(
        fs::read_link(&destination).unwrap(),
        sandbox.root.join("shared/tmux/.tmux.conf")
    );
    assert_eq!(fs::read_to_string(&destination).unwrap(), "repo\n");
}

#[test]
fn a_dry_run_never_asks_and_never_touches_unmanaged_paths() {
    let sandbox = Sandbox::new("shared\n", "shared/git/.gitconfig = ~/.gitconfig\n");
    sandbox.write("shared/git/.gitconfig", "repo\n");
    fs::write(sandbox.home.join(".gitconfig"), "live\n").unwrap();
    let (decisions, server) = decision::channel();
    let asked = std::thread::spawn(move || {
        let mut asked = 0;
        while let Some(request) = server.next() {
            asked += 1;
            let _ = server.respond(&request, Choice::Overwrite);
        }
        asked
    });
    let planning = SyncCli {
        dry_run: true,
        ..cli()
    };
    let result = engine::reconcile(
        &sandbox.context,
        "test",
        &planning,
        &decisions,
        &VecSink::default(),
    );
    drop(decisions);
    assert!(result.is_err());
    assert_eq!(asked.join().expect("responder"), 0);
    assert_eq!(
        fs::read_to_string(sandbox.home.join(".gitconfig")).unwrap(),
        "live\n"
    );
}

#[cfg(unix)]
#[test]
fn missing_link_index_discovers_existing_links_with_a_bounded_scan() {
    let sandbox = Sandbox::new("shared\n", "shared/git/.gitconfig = ~/.gitconfig\n");
    sandbox.write("shared/git/.gitconfig", "repo\n");
    let stale = sandbox.home.join(".config/stale-link");
    std::os::unix::fs::symlink(sandbox.root.join("shared/removed"), &stale).unwrap();
    assert!(!sandbox.context.root_config.join("links").exists());
    sandbox.sync(&cli()).expect("initial sync");
    assert!(fs::symlink_metadata(stale).is_err());
    let index = fs::read_to_string(sandbox.context.root_config.join("links")).unwrap();
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
fn folded_merge_package_rebuilds_only_eligible_children() {
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
    let sandbox = Sandbox::new(
        "shared\nmacos\n",
        "shared/tool = ~/.config/tool\nmacos/tool = ~/.config/tool\n",
    );
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
    let sandbox = Sandbox::new("shared\n", "shared/app = ~/.config/app\n");
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
        sandbox.context.root_config.join("profile"),
        sandbox.context.root_config.join("overrides"),
        sandbox.context.root_config.join("links"),
    ];
    state_files.extend(
        fs::read_dir(sandbox.context.root_config.join("merge"))
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
        assert_eq!(
            fs::metadata(&path).unwrap().modified().unwrap(),
            before,
            "{}",
            path.display()
        );
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

    let sandbox = Sandbox::new("shared\n", "shared/credentials = ~/.config/credentials\n");
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

#[cfg(unix)]
#[test]
fn an_edited_secret_is_restored_when_the_prompt_is_answered() {
    let sandbox = Sandbox::new("shared\n", "shared/credentials = ~/.config/credentials\n");
    sandbox.directory("shared/credentials");
    sandbox.write("shared/credentials/.secret", "");
    sandbox.write("shared/credentials/token.tmpl", "literal-token\n");
    sandbox.sync(&cli()).expect("materialize secret");
    let destination = sandbox.home.join(".config/credentials/token");
    fs::write(&destination, "edited\n").unwrap();
    assert!(sandbox.sync(&cli()).is_err());
    assert_eq!(fs::read_to_string(&destination).unwrap(), "edited\n");
    let restored = sandbox
        .sync_answering(&cli(), Some(Choice::Overwrite), &VecSink::default())
        .expect("restored secret");
    assert_eq!(restored.secrets, 1);
    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "literal-token\n"
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_secret_destination_is_replaced_once_approved() {
    let sandbox = Sandbox::new("shared\n", "shared/credentials = ~/.config/credentials\n");
    sandbox.directory("shared/credentials");
    sandbox.write("shared/credentials/.secret", "");
    sandbox.write("shared/credentials/token.tmpl", "literal-token\n");
    let destination = sandbox.home.join(".config/credentials/token");
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    let elsewhere = sandbox.home.join("elsewhere");
    fs::write(&elsewhere, "elsewhere\n").unwrap();
    std::os::unix::fs::symlink(&elsewhere, &destination).unwrap();
    assert!(sandbox.sync(&cli()).is_err());
    sandbox
        .sync_answering(&cli(), Some(Choice::Overwrite), &VecSink::default())
        .expect("replaced symlinked secret");
    assert!(
        !fs::symlink_metadata(&destination)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "literal-token\n"
    );
    assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "elsewhere\n");
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
    assert!(!sandbox.context.root_config.join("links").exists());
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
    let preview = sandbox.package_docs(&["--dry-run"]);
    assert!(preview.success(), "{preview:?}");
    let report: serde_json::Value = serde_json::from_str(&preview.stdout).unwrap();
    assert_eq!(report["changes"][0]["path"], "PACKAGES.md");
    assert_eq!(
        fs::read_to_string(&sandbox.context.packages_doc).unwrap(),
        "stale\n"
    );
    let generated = sandbox.package_docs(&[]);
    assert!(generated.success(), "{generated:?}");
    let config = fs::read_to_string(&sandbox.context.packages_config).unwrap();
    let document = fs::read_to_string(&sandbox.context.packages_doc).unwrap();
    assert!(config.contains("alpha  = First package"));
    assert!(config.contains("tool  = Custom tool"));
    assert!(document.contains("- `alpha` — First package"));
    let checked = sandbox.package_docs(&["--check"]);
    assert!(checked.success(), "{checked:?}");
    let report: serde_json::Value = serde_json::from_str(&checked.stdout).unwrap();
    assert!(report["changes"].as_array().unwrap().is_empty());
}

#[test]
fn invalid_package_metadata_never_rewrites_generated_artifacts() {
    let sandbox = Sandbox::new("shared\n", "");
    sandbox.directory("shared/tool");
    fs::write(&sandbox.context.packages_config, [0xff]).unwrap();
    fs::write(&sandbox.context.packages_doc, "preserve me\n").unwrap();
    let result = sandbox.package_docs(&[]);
    assert!(!result.success());
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
    let result = sandbox.package_docs(&[]);
    assert!(!result.success());
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
    let temporary = TempDir::new().expect("temporary home");
    let root_config = root.join("config");
    let context = Context::new(
        root,
        temporary.path().to_path_buf(),
        root_config,
        temporary.path().join(".config"),
    )
    .expect("repository context");
    let groups = packages::package_groups(&context).unwrap();
    packages::validate_packages(&context, &groups).unwrap();
    let metadata = packages::load_metadata(&context.packages_config).unwrap();
    let (config, document) = packages::render(&context, &groups, &metadata).unwrap();
    assert_eq!(
        config,
        fs::read_to_string(&context.packages_config).unwrap()
    );
    assert_eq!(document, fs::read_to_string(&context.packages_doc).unwrap());
}
