//! What the table says, what the walk finds, and where the configs come from.
//!
//! The parts that talk to other programs are checked in `tests/cli.rs`, where
//! there is a `PATH` to control; what is here is settled without one.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use workstation::Answer;

use super::*;
use configs::Source;
use lang::{Drift, Feed, LANGS, Lang, Mode};
use run::Ran;

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

/// The whole walk rests on this: a file is sorted into a row by its extension
/// alone, so an extension in two rows would make that sorting a coin toss.
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

/// An extension written by another machine still reaches its tool.
#[test]
fn the_extension_is_read_without_regard_to_case() {
    assert_eq!(Lang::of(Path::new("Data.JSON")), Some(Lang::Web));
}

/// Both rewrite the same file, so the order is the whole point of modelling
/// languages rather than tools.
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

/// A write run formats. Verifying and linting are what `--check` is for, and
/// a linter in a write run would report on a tree it had just rewritten.
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

/// clippy is a question about the code, not about whether it is formatted.
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

/// `gofmt -l` and `goimports -l` exit 0 whether or not the files are
/// formatted, so reading their status would call every Go tree clean.
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

/// taplo drowns its own findings in an INFO line naming every file it
/// resolved. Nothing else installed here does, so nothing else is touched.
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

/// A tool turned down past the level it reports at has been silenced, not
/// quietened. taplo writes its findings at ERROR, so `warn` is the floor.
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

/// `cargo fmt` takes a manifest rather than a file list; every other row
/// takes the files.
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

/// The repository keeps biome's copy under a name that will not shadow a
/// project's own when it is linked into `$HOME`; `biome.json` is the only
/// name biome itself reads.
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
fn the_walk_finds_only_the_files_some_row_owns() {
    let root = tree(&["a.py", "b.rs", "notes.md", "sub/c.lua", "LICENSE"]);
    let found = walk::walk(root.path());
    assert_eq!(shown(&found.files), ["a.py", "b.rs", "sub/c.lua"]);
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

/// Never following one is what keeps a link loop from being a hang, and what
/// keeps a run aimed at this tree from rewriting another one.
#[test]
fn a_symlinked_directory_is_not_followed() {
    let root = tree(&["here/a.py", "elsewhere/b.py"]);
    std::os::unix::fs::symlink(root.path().join("elsewhere"), root.path().join("here/link"))
        .unwrap();
    let found = walk::walk(root.path().join("here").as_path());
    assert_eq!(shown(&found.files), ["a.py"]);
}

/// A file named outright still reaches through a link; a walk does not.
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

/// A lockfile is written by a resolver, not by a person. Reformatting one
/// picks a fight with whatever wrote it: the next install puts it back, and
/// every run in between reports drift nobody introduced.
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

/// By name wherever it sits, the same way the skip list works.
#[test]
fn a_lockfile_is_left_alone_however_deep_it_is() {
    let root = tree(&["apps/web/deep/package-lock.json", "apps/web/deep/app.json"]);
    let found = walk::walk(root.path());
    assert_eq!(shown(&found.files), ["apps/web/deep/app.json"]);
    assert_eq!(shown(&found.lockfiles), ["apps/web/deep/package-lock.json"]);
}

/// Ten of the thirteen end in an extension no row owns, so they were never
/// candidates to begin with; the list names them anyway, against the day a
/// row grows one.
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

/// The reason this crate shells out to git instead of reading `.gitignore`
/// itself: ignore rules do not apply to paths git already tracks, and a
/// reader without the index cannot know that. This repository's own
/// `.gitignore` has `**/lua` in it and forty tracked `.lua` files under it.
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
    assert_eq!(shown(&kept), ["kept.py", "lua/tracked.lua"]);
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

// -------------------------------------------------------------- the driver

#[test]
fn a_language_with_no_files_gets_no_row() {
    let work = run::sort(vec!["a.py".into(), "b.py".into(), "c.lua".into()]);
    assert_eq!(work.len(), 2);
    assert_eq!(work[0].0, Lang::Python);
    assert_eq!(work[1].0, Lang::Lua);
}

/// Rows come back in table order however the threads finished, so two runs
/// over one tree read the same.
#[test]
fn the_rows_are_reported_in_table_order() {
    let work = run::sort(vec!["z.lua".into(), "a.py".into(), "m.toml".into()]);
    let root = tempfile::tempdir().unwrap();
    let done = run::run(root.path(), work, Mode::Check, false);
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
    let error = run::invoke(root.path(), &step, &[]).unwrap_err();
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

/// The shallowest ancestor is the workspace root, which is what makes
/// `--all` format every crate in the workspace whatever the target was.
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
        Mode::Write,
        false,
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

/// Probing for `.git` would name whatever repository the run is standing in.
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

/// A typo in the variable must not quietly become somebody else's checkout.
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

/// `cargo test` runs the binary out of `scripts/rust/target`, which is inside
/// the checkout even when the working directory is not.
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

/// A project can be a Rust project before it has a single `.rs` file.
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

/// The wording says which of the two things is about to happen, so `a` is
/// never a blind overwrite.
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

/// A non-interactive `--add` copies nothing rather than everything.
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

/// The whole difference between the two: `--sync` refreshes and never
/// introduces.
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

// ---------------------------------------------------------------- the report

/// A row where nothing ran must not read as a row where everything passed.
#[test]
fn a_row_whose_every_tool_is_missing_does_not_claim_success() {
    let ran = Ran {
        lang: Lang::Yaml,
        files: 2,
        missing: vec!["yamlfmt", "yamllint"],
        findings: false,
        failed: false,
        ran: 0,
        note: None,
        output: String::new(),
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
        lang: Lang::Go,
        files: 1,
        missing: vec!["goimports"],
        findings: false,
        failed: false,
        ran: 1,
        note: None,
        output: String::new(),
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
            lang: Lang::Python,
            files: 3,
            missing: Vec::new(),
            findings: true,
            failed: false,
            ran: 2,
            note: None,
            output: String::new(),
        },
        Ran {
            lang: Lang::Sql,
            files: 1,
            missing: vec!["sqlfluff"],
            findings: false,
            failed: false,
            ran: 0,
            note: None,
            output: String::new(),
        },
    ];
    assert_eq!(
        render::tally(&done, Mode::Check),
        "4 files checked, 1 with findings, 1 not installed"
    );
}
