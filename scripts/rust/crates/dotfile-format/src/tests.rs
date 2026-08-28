
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use workstation::Answer;

use super::*;
use configs::Source;
use lang::{Drift, Feed, LANGS, Lang, Mode};
use run::{Plan, Ran};

fn plan(mode: Mode) -> Plan<'static> {
    Plan {
        mode,
        verbose: false,
        injected: &[],
    }
}

fn row(lang: Lang, files: usize) -> Ran {
    Ran {
        lang,
        files,
        missing: Vec::new(),
        findings: false,
        failed: false,
        ran: 1,
        note: None,
        output: String::new(),
        blamed: Vec::new(),
    }
}

fn tree(lines: &[&str]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for line in lines {
        let path = root.path().join(line);
        if line.ends_with('/') {
            fs::create_dir_all(&path).unwrap();
            continue;
        }
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "").unwrap();
    }
    root
}

fn shown(files: &[std::path::PathBuf]) -> Vec<String> {
    files
        .iter()
        .map(|path| path.display().to_string())
        .collect()
}

// ---------------------------------------------------------------- the table

#[test]
fn every_extension_belongs_to_exactly_one_language() {
    let mut owner: HashMap<&str, Lang> = HashMap::new();
    for lang in LANGS {
        for extension in lang.extensions() {
            if let Some(first) = owner.insert(extension, lang) {
                panic!(
                    "{extension} is owned by both {} and {}",
                    first.name(),
                    lang.name()
                );
            }
        }
    }
    assert_eq!(Lang::of(Path::new("a/b.rs")), Some(Lang::Rust));
    assert_eq!(
        Lang::of(Path::new("config/targets.dotfile")),
        Some(Lang::Dotfmt)
    );
    assert_eq!(Lang::of(Path::new("README.md")), None);
    assert_eq!(Lang::of(Path::new(".editorconfig")), None);
}

#[test]
fn the_extension_is_read_without_regard_to_case() {
    assert_eq!(Lang::of(Path::new("Data.JSON")), Some(Lang::Web));
}

#[test]
fn go_runs_goimports_before_gofmt_in_one_sequence() {
    let steps = Lang::Go.steps(Mode::Write);
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].program, "goimports");
    assert_eq!(steps[1].program, "gofmt");
}

#[test]
fn checking_yaml_runs_the_formatter_before_the_linter() {
    let steps = Lang::Yaml.steps(Mode::Check);
    assert_eq!(steps[0].program, "yamlfmt");
    assert_eq!(steps[1].program, "yamllint");
}

#[test]
fn checking_python_verifies_the_formatter_and_then_lints() {
    let steps = Lang::Python.steps(Mode::Check);
    assert_eq!(steps[0].args, ["format", "--check"]);
    assert_eq!(steps[1].args, ["check"]);
}

#[test]
fn a_write_run_never_verifies_and_never_lints() {
    for lang in LANGS {
        for step in lang.steps(Mode::Write) {
            assert!(
                !step.program.ends_with("lint"),
                "{} runs {} in a write run",
                lang.name(),
                step.program
            );
            for argument in step.args {
                assert!(
                    !argument.contains("lint") && !argument.contains("check"),
                    "{} passes {argument} in a write run",
                    lang.name()
                );
            }
        }
    }
}

#[test]
fn no_row_ever_runs_clippy() {
    for lang in LANGS {
        for mode in [Mode::Write, Mode::Check] {
            for step in lang.steps(mode) {
                assert!(!step.args.contains(&"clippy"));
                assert_ne!(step.program, "clippy");
            }
        }
    }
}

#[test]
fn only_gofmt_and_goimports_report_drift_by_listing() {
    for lang in LANGS {
        for step in lang.steps(Mode::Check) {
            let listing = step.drift == Drift::Listing;
            assert_eq!(
                listing,
                matches!(step.program, "gofmt" | "goimports"),
                "{} is wrong about how {} reports drift",
                lang.name(),
                step.program
            );
        }
    }
}

#[test]
fn taplo_is_the_one_row_whose_logging_is_turned_down() {
    for lang in LANGS {
        for mode in [Mode::Write, Mode::Check] {
            for step in lang.steps(mode) {
                assert_eq!(
                    step.env.is_empty(),
                    step.program != "taplo",
                    "{} sets the wrong environment for {}",
                    lang.name(),
                    step.program
                );
            }
        }
    }
    for step in Lang::Toml.steps(Mode::Check) {
        assert_eq!(step.env, [("RUST_LOG", "warn")]);
    }
}

#[test]
fn no_row_is_turned_down_below_the_level_its_findings_are_written_at() {
    for lang in LANGS {
        for mode in [Mode::Write, Mode::Check] {
            for step in lang.steps(mode) {
                for (name, level) in step.env {
                    assert_eq!(*name, "RUST_LOG");
                    assert_eq!(
                        *level,
                        "warn",
                        "{} would hide what {} found",
                        lang.name(),
                        step.program
                    );
                }
            }
        }
    }
}

#[test]
fn rust_is_the_only_row_that_is_not_handed_files() {
    for lang in LANGS {
        for mode in [Mode::Write, Mode::Check] {
            for step in lang.steps(mode) {
                assert_eq!(step.feed == Feed::Manifests, lang == Lang::Rust);
            }
        }
    }
}

#[test]
fn biome_is_the_only_config_that_lands_under_a_different_name() {
    for lang in LANGS {
        if let Some((from, name)) = lang.config() {
            assert_eq!(from != name, lang == Lang::Web, "{}", lang.name());
        }
    }
    assert_eq!(
        Lang::Web.config(),
        Some(("biome.global.json", "biome.json"))
    );
    assert_eq!(Lang::Go.config(), None);
}

// ----------------------------------------------------------------- the walk

#[test]
fn the_walk_offers_every_file_it_reaches() {
    let root = tree(&["a.py", "b.rs", "notes.md", "sub/c.lua", "LICENSE"]);
    let found = walk::walk(root.path());
    assert_eq!(
        shown(&found.files),
        ["LICENSE", "a.py", "b.rs", "notes.md", "sub/c.lua"]
    );
}

#[test]
fn the_skip_list_is_not_walked() {
    let root = tree(&[
        "keep.py",
        "node_modules/pkg/index.js",
        "target/debug/build.rs",
        ".git/hooks/pre-commit.sh",
        ".venv/lib/x.py",
        "__pycache__/y.py",
    ]);
    assert_eq!(shown(&walk::walk(root.path()).files), ["keep.py"]);
}

#[test]
fn a_symlinked_directory_is_not_followed() {
    let root = tree(&["here/a.py", "elsewhere/b.py"]);
    std::os::unix::fs::symlink(root.path().join("elsewhere"), root.path().join("here/link"))
        .unwrap();
    let found = walk::walk(root.path().join("here").as_path());
    assert_eq!(shown(&found.files), ["a.py"]);
}

#[test]
fn a_symlinked_file_is_not_picked_up_by_the_walk() {
    let root = tree(&["here/a.py", "elsewhere/b.py"]);
    std::os::unix::fs::symlink(
        root.path().join("elsewhere/b.py"),
        root.path().join("here/link.py"),
    )
    .unwrap();
    assert_eq!(
        shown(&walk::walk(root.path().join("here").as_path()).files),
        ["a.py"]
    );
}

#[test]
fn a_generated_lockfile_is_left_where_it_is() {
    let root = tree(&[
        "lazy-lock.json",
        "package-lock.json",
        "pnpm-lock.yaml",
        "settings.json",
        "ci.yaml",
    ]);
    let found = walk::walk(root.path());
    assert_eq!(shown(&found.files), ["ci.yaml", "settings.json"]);
    assert_eq!(
        shown(&found.lockfiles),
        ["lazy-lock.json", "package-lock.json", "pnpm-lock.yaml"]
    );
}

#[test]
fn a_lockfile_is_left_alone_however_deep_it_is() {
    let root = tree(&["apps/web/deep/package-lock.json", "apps/web/deep/app.json"]);
    let found = walk::walk(root.path());
    assert_eq!(shown(&found.files), ["apps/web/deep/app.json"]);
    assert_eq!(shown(&found.lockfiles), ["apps/web/deep/package-lock.json"]);
}

#[test]
fn no_name_in_the_lockfile_list_ever_reaches_a_tool() {
    let root = tree(&walk::LOCKFILES);
    let found = walk::walk(root.path());
    assert!(found.files.is_empty(), "{:?}", shown(&found.files));
    assert_eq!(walk::LOCKFILES.len(), 13);
}

#[test]
fn the_depth_cap_stops_the_walk_and_says_so() {
    let root = tempfile::tempdir().unwrap();
    let mut deep = root.path().to_path_buf();
    for step in 0..walk::MAX_DEPTH + 2 {
        deep.push(format!("d{step}"));
    }
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("buried.py"), "").unwrap();
    let found = walk::walk(root.path());
    assert!(found.files.is_empty());
    assert!(found.deep > 0);
}

#[test]
fn a_directory_that_cannot_be_read_is_counted_rather_than_ignored() {
    let root = tree(&["a.py", "locked/b.py"]);
    let locked = root.path().join("locked");
    fs::set_permissions(
        &locked,
        <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o000),
    )
    .unwrap();
    let found = walk::walk(root.path());
    fs::set_permissions(
        &locked,
        <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )
    .unwrap();
    assert_eq!(shown(&found.files), ["a.py"]);
    assert_eq!(found.unreadable, 1);
}

#[test]
fn a_tracked_file_survives_an_ignore_rule_that_matches_it() {
    let root = tree(&["lua/tracked.lua", "other/lua/untracked.lua", "kept.py"]);
    fs::write(root.path().join(".gitignore"), "**/lua\n").unwrap();
    for arguments in [vec!["init", "-q"], vec!["add", "-f", "lua/tracked.lua"]] {
        let status = std::process::Command::new("git")
            .args(arguments)
            .current_dir(root.path())
            .status()
            .unwrap();
        assert!(status.success());
    }
    let found = walk::walk(root.path());
    let kept = walk::drop_ignored(root.path(), found.files);
    assert_eq!(shown(&kept), [".gitignore", "kept.py", "lua/tracked.lua"]);
}

#[test]
fn outside_a_work_tree_nothing_is_dropped() {
    let root = tree(&["a.py"]);
    let found = walk::walk(root.path());
    assert_eq!(
        shown(&walk::drop_ignored(root.path(), found.files)),
        ["a.py"]
    );
}

// ------------------------------------------------- files encrypted at rest

const SEALED_VALUE: &str =
    "password: ENC[AES256_GCM,data:qWuPqA==,iv:wtj3wg=,tag:6rqTIQ==,type:str]";

fn holding(body: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("f"), body).unwrap();
    root
}

fn sealed(body: &str) -> bool {
    let root = holding(body);
    !walk::encrypted(root.path(), &["f".into()]).is_empty()
}

#[test]
fn a_sops_document_is_recognised() {
    assert!(sealed(&format!(
        "{SEALED_VALUE}\nsops:\n    version: 3.13.3\n"
    )));
}

#[test]
fn the_configuration_that_says_what_to_encrypt_is_not_itself_encrypted() {
    // `.sops.yaml`: the recipients to encrypt *for*, and nothing encrypted.
    assert!(!sealed(
        "creation_rules:\n  - age: age15wjewk6yjs5vsezah0sa9vz3gyl569eexwj74l8dvrc2vlsxuq3q7hq52d\n"
    ));
}

#[test]
fn source_that_merely_names_the_marker_is_not_a_secret() {
    assert!(!sealed("SOPS_MARKER = \"ENC[AES256_GCM\"\n"));
    assert!(!sealed("const CIPHERTEXT: &str = \"ENC[AES256_GCM\";\n"));
    assert!(!sealed("assert \"ENC[AES256_GCM\" in sealed.read_text()\n"));
    // A test building half of one is still not one.
    assert!(!sealed(
        "stage(root, f\"data: ENC[AES256_GCM,data:xx] {TOKEN}\")\n"
    ));
}

#[test]
fn the_metadata_block_is_recognised_in_every_format_sops_writes() {
    assert!(sealed(
        "{\n\t\"a\": \"x\",\n\t\"sops\": {\n\t\t\"version\": \"3.13.3\"\n\t}\n}\n"
    ));
    assert!(sealed("A=x\nsops_version=3.13.3\n"));
    assert!(sealed("[a]\nb=x\n[sops]\nversion=3.13.3\n"));
}

#[test]
fn a_nested_sops_key_is_not_the_metadata_block() {
    assert!(!sealed("tools:\n  sops:\n    enabled: true\n"));
}

#[test]
fn a_block_past_the_head_of_a_long_file_is_still_found() {
    let mut body = String::new();
    for nth in 0..2000 {
        body.push_str(&format!("key_{nth}: plaintext value number {nth}\n"));
    }
    assert!(body.len() > walk::SNIFF * 2);
    body.push_str("sops:\n    version: 3.13.3\n");
    assert!(sealed(&body));
}

#[test]
fn a_long_plaintext_file_is_left_alone_by_the_sniff() {
    let body: String = (0..2000).map(|nth| format!("key_{nth}: {nth}\n")).collect();
    assert!(!sealed(&body));
}

#[test]
fn an_encrypted_file_is_taken_out_of_whatever_row_it_was_in() {
    let root = tempfile::tempdir().unwrap();
    let body = format!("{SEALED_VALUE}\nsops:\n    version: 3.13.3\n");
    fs::write(root.path().join("secrets.yaml"), &body).unwrap();
    fs::write(root.path().join("ci.yaml"), "a: 1\n").unwrap();
    fs::write(root.path().join("creds.json"), &body).unwrap();

    let mut work = run::sort(vec![
        "secrets.yaml".into(),
        "ci.yaml".into(),
        "creds.json".into(),
    ]);
    let gone = walk::drop_encrypted(root.path(), &mut work);
    assert_eq!(shown(&gone), ["creds.json", "secrets.yaml"]);
    // The web row held nothing else, so it is gone; yaml keeps the one file
    // that was not a secret.
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].0, Lang::Yaml);
    assert_eq!(shown(&work[0].1), ["ci.yaml"]);
}

// -------------------------------------------------------------- the driver

#[test]
fn sorting_never_builds_the_dotfmt_row() {
    let work = run::sort(vec![
        "a.conf".into(),
        "b.dotfile".into(),
        "c.py".into(),
        "notes.md".into(),
    ]);
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].0, Lang::Python);
}

#[test]
fn the_fallback_row_is_the_three_extensions_dotfmt_has_always_had() {
    let files: Vec<std::path::PathBuf> = ["a.conf", "b.config", "c.dotfile", "d.py", "LICENSE"]
        .iter()
        .map(Into::into)
        .collect();
    assert_eq!(
        shown(&run::by_extension(&files)),
        ["a.conf", "b.config", "c.dotfile"]
    );
    assert_eq!(Lang::Dotfmt.extensions(), ["conf", "config", "dotfile"]);
}

#[test]
fn dotfmts_answer_becomes_the_first_row_and_an_empty_answer_becomes_none() {
    let work = run::with_dotfmt(run::sort(vec!["c.py".into()]), vec!["a.conf".into()]);
    assert_eq!(work[0].0, Lang::Dotfmt);
    assert_eq!(shown(&work[0].1), ["a.conf"]);
    assert_eq!(work[1].0, Lang::Python);

    let none = run::with_dotfmt(run::sort(vec!["c.py".into()]), Vec::new());
    assert_eq!(none.len(), 1);
    assert_eq!(none[0].0, Lang::Python);
}

// -------------------------------------------------- what a tool pointed at

#[test]
fn the_files_a_tool_named_are_recognised_however_it_decorated_them() {
    let files: Vec<std::path::PathBuf> = [
        "shared/zsh/conf.d/90-utils.zsh",
        "shared/vscode/keybindings.json",
        "a.yaml",
        "untouched.py",
    ]
    .iter()
    .map(Into::into)
    .collect();
    let said = "shared/zsh/conf.d/90-utils.zsh:376:24: not a valid parameter expansion\n\
                shared/vscode/keybindings.json:1:1 parse ━━━\n\
                a.yaml:\n";
    assert_eq!(
        run::blamed(said, Path::new("/nowhere"), &files),
        [
            "shared/zsh/conf.d/90-utils.zsh",
            "shared/vscode/keybindings.json",
            "a.yaml"
        ]
    );
}

#[test]
fn a_tool_that_names_a_file_inside_a_field_is_read_the_same_way() {
    let files: Vec<std::path::PathBuf> = vec!["theme/roles.toml".into()];
    assert_eq!(
        run::blamed(
            "ERROR the file is not properly formatted \
             path=\"/home/x/repo/theme/roles.toml\"\n",
            Path::new("/home/x/repo"),
            &files
        ),
        ["theme/roles.toml"]
    );
}

#[test]
fn a_tool_that_answers_in_absolute_paths_is_read_the_same_way() {
    let files: Vec<std::path::PathBuf> = vec!["crates/dotfmt/src/select.rs".into()];
    assert_eq!(
        run::blamed(
            "Diff in /home/x/repo/crates/dotfmt/src/select.rs:161:\n",
            Path::new("/home/x/repo"),
            &files
        ),
        ["crates/dotfmt/src/select.rs"]
    );
}

#[test]
fn a_file_is_only_named_when_the_whole_path_matches() {
    let files: Vec<std::path::PathBuf> = vec!["src/a.py".into()];
    let here = Path::new("/nowhere");
    assert!(run::blamed("would reformat src/data.py\n", here, &files).is_empty());
    assert_eq!(
        run::blamed("would reformat src/a.py\n", here, &files),
        ["src/a.py"]
    );
}

#[test]
fn a_file_named_twice_is_named_once() {
    let files: Vec<std::path::PathBuf> = vec!["a.py".into()];
    assert_eq!(
        run::blamed("a.py:1:1 x\na.py:9:2 y\n", Path::new("/nowhere"), &files),
        ["a.py"]
    );
}

#[test]
fn a_language_with_no_files_gets_no_row() {
    let work = run::sort(vec!["a.py".into(), "b.py".into(), "c.lua".into()]);
    assert_eq!(work.len(), 2);
    assert_eq!(work[0].0, Lang::Python);
    assert_eq!(work[1].0, Lang::Lua);
}

#[test]
fn the_rows_are_reported_in_table_order() {
    let work = run::sort(vec!["z.lua".into(), "a.py".into(), "m.toml".into()]);
    let root = tempfile::tempdir().unwrap();
    let done = run::run(root.path(), work, &plan(Mode::Check));
    let order: Vec<&str> = done.iter().map(|ran| ran.lang.name()).collect();
    assert_eq!(order, ["python", "lua", "toml"]);
}

#[test]
fn a_program_that_is_not_installed_is_a_missing_program_and_not_an_error() {
    let root = tempfile::tempdir().unwrap();
    let step = lang::Step {
        program: "no-such-formatter-anywhere",
        args: &[],
        env: &[],
        feed: Feed::Files,
        drift: Drift::Status,
    };
    let error = run::invoke(root.path(), &step, &[], &[]).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn chunking_splits_at_512_and_keeps_the_order() {
    let files: Vec<std::path::PathBuf> = (0..1100).map(|nth| format!("f{nth}.py").into()).collect();
    let batches = run::batches(&files);
    assert_eq!(batches.len(), 3);
    assert_eq!(batches[0].len(), run::CHUNK);
    assert_eq!(batches[1].len(), run::CHUNK);
    assert_eq!(batches[2].len(), 1100 - 2 * run::CHUNK);
    assert_eq!(batches[0][0], "f0.py");
    assert_eq!(batches[1][0], format!("f{}.py", run::CHUNK).as_str());
}

#[test]
fn the_shallowest_cargo_toml_above_a_file_is_the_one_used() {
    let root = tree(&[
        "Cargo.toml",
        "crates/one/Cargo.toml",
        "crates/one/src/main.rs",
    ]);
    let batches = run::manifest_arguments(root.path(), &["crates/one/src/main.rs".into()]);
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0][0], "--manifest-path");
    assert_eq!(
        batches[0][1],
        root.path().join("Cargo.toml").into_os_string()
    );
}

#[test]
fn rust_files_with_no_manifest_above_them_are_reported_rather_than_run() {
    let root = tree(&["stray.rs"]);
    let done = run::run(
        root.path(),
        run::sort(vec!["stray.rs".into()]),
        &plan(Mode::Write),
    );
    assert_eq!(
        done[0].note.as_deref(),
        Some("no Cargo.toml above these files")
    );
    assert!(!done[0].failed);
}

// ---------------------------------------------------------------- the configs

fn checkout() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("environment")).unwrap();
    fs::create_dir_all(root.path().join("config")).unwrap();
    fs::write(root.path().join("config/targets.dotfile"), "").unwrap();
    fs::create_dir_all(root.path().join("shared/tools")).unwrap();
    for (name, _) in configs::EMBEDDED {
        fs::write(
            root.path().join("shared/tools").join(name),
            format!("live {name}\n"),
        )
        .unwrap();
    }
    root
}

#[test]
fn the_marker_predicate_wants_both_environment_and_the_targets_file() {
    let root = tempfile::tempdir().unwrap();
    assert!(!configs::is_root(root.path()));
    fs::create_dir(root.path().join("environment")).unwrap();
    assert!(!configs::is_root(root.path()));
    fs::create_dir(root.path().join("config")).unwrap();
    fs::write(root.path().join("config/targets.dotfile"), "").unwrap();
    assert!(configs::is_root(root.path()));
    fs::create_dir(root.path().join(".git")).unwrap();
    assert!(configs::is_root(root.path()));
}

#[test]
fn a_dotfile_root_that_is_not_this_repository_is_an_error_and_not_a_fallthrough() {
    let elsewhere = tempfile::tempdir().unwrap();
    let real = checkout();
    let error = configs::resolve(
        Some(elsewhere.path().to_path_buf()),
        Some(real.path().to_path_buf()),
        Some(real.path().to_path_buf()),
        Some(real.path().to_path_buf()),
    )
    .unwrap_err();
    assert!(error.contains("DOTFILE_ROOT"), "{error}");
    assert!(error.contains("environment/"), "{error}");
}

#[test]
fn dotfile_root_wins_over_everything_else() {
    let named = checkout();
    let other = checkout();
    let Ok(Source::Repo(found)) = configs::resolve(
        Some(named.path().to_path_buf()),
        Some(other.path().to_path_buf()),
        None,
        None,
    ) else {
        panic!("the named root is the one used");
    };
    assert_eq!(found, named.path());
}

#[test]
fn the_working_directory_is_climbed_before_the_binary_is_asked() {
    let near = checkout();
    let far = checkout();
    let deep = near.path().join("a/b/c");
    fs::create_dir_all(&deep).unwrap();
    let Ok(Source::Repo(found)) =
        configs::resolve(None, Some(deep), Some(far.path().to_path_buf()), None)
    else {
        panic!("the working directory's checkout is the one used");
    };
    assert_eq!(found, near.path());
}

#[test]
fn the_binary_is_climbed_when_the_working_directory_is_not_in_a_checkout() {
    let real = checkout();
    let elsewhere = tempfile::tempdir().unwrap();
    let deep = real.path().join("scripts/rust/target/debug");
    fs::create_dir_all(&deep).unwrap();
    let Ok(Source::Repo(found)) =
        configs::resolve(None, Some(elsewhere.path().to_path_buf()), Some(deep), None)
    else {
        panic!("the binary's checkout is the one used");
    };
    assert_eq!(found, real.path());
}

#[test]
fn with_no_checkout_anywhere_the_compiled_in_copies_are_the_source() {
    let elsewhere = tempfile::tempdir().unwrap();
    let source = configs::resolve(
        None,
        Some(elsewhere.path().to_path_buf()),
        Some(elsewhere.path().to_path_buf()),
        Some(elsewhere.path().to_path_buf()),
    )
    .unwrap();
    assert!(matches!(source, Source::Embedded));
}

#[test]
fn every_config_the_table_names_has_a_copy_compiled_in() {
    for lang in LANGS {
        let Some((from, _)) = lang.config() else {
            continue;
        };
        let text = configs::read(&Source::Embedded, from).unwrap();
        assert!(!text.is_empty(), "{from} is compiled in empty");
    }
    assert_eq!(configs::EMBEDDED.len(), 9);
}

#[test]
fn a_checkout_is_read_live_rather_than_from_the_copies() {
    let root = checkout();
    let source = Source::Repo(root.path().to_path_buf());
    assert_eq!(
        configs::read(&source, "ruff.toml").unwrap(),
        "live ruff.toml\n"
    );
}

#[test]
fn a_marker_file_settles_a_language_the_walk_found_nothing_of() {
    let root = tree(&["Cargo.toml", "go.mod"]);
    let langs = configs::detect(root.path(), &[]);
    assert!(langs.contains(&Lang::Rust));
    assert!(langs.contains(&Lang::Go));
    assert!(!langs.contains(&Lang::Python));
}

#[test]
fn one_file_of_a_language_is_enough_to_offer_it_a_config() {
    let root = tempfile::tempdir().unwrap();
    let langs = configs::detect(root.path(), &["src/only.py".into()]);
    assert_eq!(langs, [Lang::Python]);
}

#[test]
fn a_placement_lands_biome_under_the_only_name_biome_reads() {
    let root = tempfile::tempdir().unwrap();
    let placements = configs::placements(root.path(), &[Lang::Web, Lang::Go]);
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].from, "biome.global.json");
    assert_eq!(placements[0].to, root.path().join("biome.json"));
    assert!(!placements[0].exists);
}

#[test]
fn add_asks_about_each_file_and_copies_what_was_agreed_to() {
    let repo = checkout();
    let project = tempfile::tempdir().unwrap();
    let placements = configs::placements(project.path(), &[Lang::Python, Lang::Lua]);
    let mut answers = [Answer::Yes, Answer::No].into_iter();
    let mut asked = Vec::new();
    let done = configs::add(
        &Source::Repo(repo.path().to_path_buf()),
        &placements,
        &mut |question| {
            asked.push(question.to_string());
            answers.next()
        },
    )
    .unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].name, "ruff.toml");
    assert!(asked[0].contains("copy ruff.toml? [Y/n/a]"), "{}", asked[0]);
    assert!(project.path().join("ruff.toml").exists());
    assert!(!project.path().join("stylua.toml").exists());
}

#[test]
fn a_file_that_is_already_there_is_offered_as_a_replacement() {
    let repo = checkout();
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("ruff.toml"), "mine\n").unwrap();
    let placements = configs::placements(project.path(), &[Lang::Python]);
    let mut asked = String::new();
    configs::add(
        &Source::Repo(repo.path().to_path_buf()),
        &placements,
        &mut |question| {
            asked = question.to_string();
            Some(Answer::All)
        },
    )
    .unwrap();
    assert!(asked.contains("replace ruff.toml?"), "{asked}");
    assert_eq!(
        fs::read_to_string(project.path().join("ruff.toml")).unwrap(),
        "live ruff.toml\n"
    );
}

#[test]
fn answering_all_copies_the_rest_without_asking_again() {
    let repo = checkout();
    let project = tempfile::tempdir().unwrap();
    let placements = configs::placements(project.path(), &[Lang::Python, Lang::Lua, Lang::Toml]);
    let mut asked = 0;
    let done = configs::add(
        &Source::Repo(repo.path().to_path_buf()),
        &placements,
        &mut |_| {
            asked += 1;
            Some(Answer::All)
        },
    )
    .unwrap();
    assert_eq!(asked, 1);
    assert_eq!(done.len(), 3);
}

#[test]
fn add_stops_when_the_answers_run_out() {
    let repo = checkout();
    let project = tempfile::tempdir().unwrap();
    let placements = configs::placements(project.path(), &[Lang::Python, Lang::Lua]);
    let done = configs::add(
        &Source::Repo(repo.path().to_path_buf()),
        &placements,
        &mut |_| None,
    )
    .unwrap();
    assert!(done.is_empty());
    assert!(!project.path().join("ruff.toml").exists());
}

#[test]
fn sync_replaces_what_is_there_and_introduces_nothing() {
    let repo = checkout();
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("stylua.toml"), "stale\n").unwrap();
    let placements = configs::placements(project.path(), &[Lang::Python, Lang::Lua]);
    let done = configs::sync(&Source::Repo(repo.path().to_path_buf()), &placements).unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(
        fs::read_to_string(project.path().join("stylua.toml")).unwrap(),
        "live stylua.toml\n"
    );
    assert!(!project.path().join("ruff.toml").exists());
}

#[test]
fn a_target_with_no_config_of_its_own_is_pointed_at_this_repositorys() {
    let repo = checkout();
    let project = tempfile::tempdir().unwrap();
    let injected = configs::injections(
        &Source::Repo(repo.path().to_path_buf()),
        &project.path().canonicalize().unwrap(),
    );
    let tools = repo.path().join("shared/tools");
    let said: Vec<(&str, Vec<String>)> = injected
        .iter()
        .map(|one| {
            (
                one.program,
                one.args
                    .iter()
                    .map(|argument| argument.to_string_lossy().into_owned())
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        said,
        [
            (
                "taplo",
                vec![
                    "--config".to_string(),
                    tools.join(".taplo.toml").display().to_string()
                ]
            ),
            (
                "biome",
                vec![
                    "--config-path".to_string(),
                    tools.join("biome.global.json").display().to_string()
                ]
            ),
            (
                "stylua",
                vec![
                    "--config-path".to_string(),
                    tools.join("stylua.toml").display().to_string()
                ]
            ),
            (
                "yamllint",
                vec![
                    "-c".to_string(),
                    tools.join(".yamllint.yaml").display().to_string()
                ]
            ),
        ]
    );
}

fn injected_programs(repo: &tempfile::TempDir, root: &Path) -> Vec<&'static str> {
    configs::injections(&Source::Repo(repo.path().to_path_buf()), root)
        .iter()
        .map(|one| one.program)
        .collect()
}

#[test]
fn a_config_the_target_already_has_is_never_replaced() {
    let repo = checkout();
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join(".taplo.toml"), "mine\n").unwrap();
    assert_eq!(
        injected_programs(&repo, &project.path().canonicalize().unwrap()),
        ["biome", "stylua", "yamllint"]
    );
}

#[test]
fn a_config_above_the_target_counts_as_the_targets_own() {
    let repo = checkout();
    let above = tempfile::tempdir().unwrap();
    fs::write(above.path().join("biome.jsonc"), "{}\n").unwrap();
    let project = above.path().join("deep/inside");
    fs::create_dir_all(&project).unwrap();
    assert_eq!(
        injected_programs(&repo, &project.canonicalize().unwrap()),
        ["taplo", "stylua", "yamllint"]
    );
}

#[test]
fn the_copies_compiled_in_are_never_named_on_a_command_line() {
    let project = tempfile::tempdir().unwrap();
    assert!(configs::injections(&Source::Embedded, project.path()).is_empty());
}

#[test]
fn injection_is_keyed_by_program_and_not_by_language() {
    let repo = checkout();
    let project = tempfile::tempdir().unwrap();
    let injected = configs::injections(
        &Source::Repo(repo.path().to_path_buf()),
        &project.path().canonicalize().unwrap(),
    );
    assert!(injected.iter().any(|one| one.program == "yamllint"));
    assert!(!injected.iter().any(|one| one.program == "yamlfmt"));
}

#[test]
fn only_the_programs_that_resolve_per_file_split_a_row() {
    for injection in configs::INJECTED {
        let split = matches!(injection.scope, configs::Scope::Unclaimed(_));
        assert_eq!(
            split,
            matches!(injection.program, "stylua" | "biome"),
            "{} is wrong about how it resolves",
            injection.program
        );
    }
}

#[test]
fn a_config_below_the_root_wins_for_its_own_subtree_only() {
    let root = tree(&[
        "nvim/.stylua.toml",
        "nvim/init.lua",
        "nvim/lua/plugins/lsp.lua",
        "yazi/init.lua",
        "hammerspoon/init.lua",
    ]);
    let files: Vec<std::path::PathBuf> = [
        "nvim/init.lua",
        "nvim/lua/plugins/lsp.lua",
        "yazi/init.lua",
        "hammerspoon/init.lua",
    ]
    .iter()
    .map(Into::into)
    .collect();
    let (theirs, ours) = configs::partition(root.path(), &[".stylua.toml"], &files);
    assert_eq!(
        shown(&theirs),
        ["nvim/init.lua", "nvim/lua/plugins/lsp.lua"]
    );
    assert_eq!(shown(&ours), ["yazi/init.lua", "hammerspoon/init.lua"]);
}

// ------------------------------------- where every program's config comes from

#[test]
fn every_program_says_where_its_configuration_comes_from() {
    for program in lang::programs() {
        let Some(configured) = lang::configured(program) else {
            panic!("{program} does not say where its configuration comes from");
        };
        let injected = configs::INJECTED
            .iter()
            .any(|injection| injection.program == program);
        match configured {
            lang::Configured::Named => assert!(
                injected,
                "{program} says the run names its config, and nothing in INJECTED does"
            ),
            lang::Configured::Found(why) | lang::Configured::Gap(why) => {
                assert!(!why.is_empty(), "{program} gives no reason");
                assert!(!injected, "{program} is injected and does not say so");
            }
            lang::Configured::Nothing => {
                assert!(!injected, "{program} is injected and says it has no config");
            }
        }
    }
}

#[test]
fn every_injection_is_for_a_program_the_table_runs() {
    let programs = lang::programs();
    for injection in configs::INJECTED {
        assert!(
            programs.contains(&injection.program),
            "{} is injected and never run",
            injection.program
        );
    }
    assert_eq!(programs.len(), 12);
}

// ---------------------------------------------------------------- the report

#[test]
fn a_row_whose_every_tool_is_missing_does_not_claim_success() {
    let ran = Ran {
        missing: vec!["yamlfmt", "yamllint"],
        ran: 0,
        ..row(Lang::Yaml, 2)
    };
    let lines = render::report(&[ran], Mode::Check, &workstation::Style::plain());
    assert_eq!(
        lines[0],
        "  yaml  2 files  yamlfmt not installed, yamllint not installed"
    );
}

#[test]
fn a_row_that_ran_says_so_and_still_names_the_tool_it_was_missing() {
    let ran = Ran {
        missing: vec!["goimports"],
        ..row(Lang::Go, 1)
    };
    let lines = render::report(&[ran], Mode::Write, &workstation::Style::plain());
    assert_eq!(
        lines[0],
        "  go  1 file   formatted  goimports not installed"
    );
}

#[test]
fn a_missing_tool_is_counted_in_the_tally_apart_from_the_findings() {
    let done = vec![
        Ran {
            findings: true,
            ran: 2,
            ..row(Lang::Python, 3)
        },
        Ran {
            missing: vec!["sqlfluff"],
            ran: 0,
            ..row(Lang::Sql, 1)
        },
    ];
    assert_eq!(
        render::tally(&done, Mode::Check),
        "4 files checked, 1 with findings, 1 not installed"
    );
}

// ------------------------------------------------------------- the summary

fn summary(done: &[Ran], mode: Mode) -> Vec<String> {
    render::summary(done, mode, &workstation::Style::plain())
}

#[test]
fn a_run_with_nothing_to_report_is_one_line() {
    let done = vec![row(Lang::Python, 120), row(Lang::Lua, 48)];
    assert_eq!(summary(&done, Mode::Write), ["168 / 168 files formatted"]);
}

#[test]
fn a_check_run_says_clean_rather_than_formatted() {
    assert_eq!(
        summary(&[row(Lang::Toml, 42)], Mode::Check),
        ["42 / 42 files clean"]
    );
}

#[test]
fn a_failed_provider_gets_a_line_and_its_files_leave_the_count() {
    let done = vec![
        Ran {
            failed: true,
            ..row(Lang::Web, 40)
        },
        Ran {
            failed: true,
            ..row(Lang::Yaml, 5)
        },
        row(Lang::Python, 123),
    ];
    assert_eq!(
        summary(&done, Mode::Write),
        [
            "web   40 files  failed",
            "yaml   5 files  failed",
            "123 / 168 files formatted",
        ]
    );
}

#[test]
fn drift_names_the_provider_in_the_same_shape() {
    let done = vec![
        Ran {
            findings: true,
            ..row(Lang::Python, 3)
        },
        row(Lang::Lua, 7),
    ];
    assert_eq!(
        summary(&done, Mode::Check),
        ["python  3 files  findings", "7 / 10 files clean"]
    );
}

#[test]
fn a_failed_provider_names_the_files_it_fell_over_on() {
    let done = vec![Ran {
        failed: true,
        blamed: vec!["shared/zsh/conf.d/90-utils.zsh".to_string()],
        ..row(Lang::Shell, 34)
    }];
    assert_eq!(
        summary(&done, Mode::Write),
        [
            "shell  34 files  failed",
            "  shared/zsh/conf.d/90-utils.zsh",
            "0 / 34 files formatted",
        ]
    );
}

#[test]
fn the_files_named_under_a_provider_are_capped() {
    let blamed: Vec<String> = (0..9).map(|nth| format!("f{nth}.py")).collect();
    let done = vec![Ran {
        failed: true,
        blamed,
        ..row(Lang::Python, 9)
    }];
    let lines = summary(&done, Mode::Write);
    assert_eq!(lines[1], "  f0.py");
    assert_eq!(lines[5], "  f4.py");
    assert_eq!(lines[6], "  … and 4 more");
    assert_eq!(lines[7], "0 / 9 files formatted");
}

#[test]
fn a_failure_that_names_no_file_shows_what_the_tool_said() {
    let done = vec![Ran {
        failed: true,
        output: "\ndotfmt --owns: unexpected argument\n".to_string(),
        ..row(Lang::Dotfmt, 0)
    }];
    assert_eq!(
        summary(&done, Mode::Write),
        [
            "dotfmt  0 files  failed",
            "  dotfmt --owns: unexpected argument",
            "0 / 0 files formatted",
        ]
    );
}

#[test]
fn a_row_whose_every_tool_is_missing_is_left_out_of_the_count() {
    let done = vec![
        row(Lang::Python, 10),
        Ran {
            missing: vec!["sqlfluff"],
            ran: 0,
            ..row(Lang::Sql, 4)
        },
    ];
    assert_eq!(summary(&done, Mode::Write), ["10 / 10 files formatted"]);
}

#[test]
fn when_nothing_ran_at_all_the_missing_tools_are_the_report() {
    let done = vec![
        Ran {
            missing: vec!["yamlfmt", "yamllint"],
            ran: 0,
            ..row(Lang::Yaml, 2)
        },
        Ran {
            missing: vec!["yamlfmt"],
            ran: 0,
            ..row(Lang::Sql, 1)
        },
    ];
    assert_eq!(
        summary(&done, Mode::Write),
        ["yamlfmt, yamllint not installed"]
    );
}
