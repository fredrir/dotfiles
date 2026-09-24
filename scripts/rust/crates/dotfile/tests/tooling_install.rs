#![forbid(unsafe_code)]
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use dotfile_cli::context::Context;
use dotfile_cli::event::VecSink;
use dotfile_cli::tooling::catalog::{Language, Toolchain};
use dotfile_cli::tooling::{digest, install};
use testkit::{TempDir, executable, tree_pairs};

fn sandbox() -> TempDir {
    tree_pairs(&[
        ("config/targets.dotfile", ""),
        (
            "scripts/rust/Cargo.toml",
            "[workspace]\nmembers = ['crates/tool', 'crates/pair', 'crates/library']\n",
        ),
        (
            "scripts/rust/crates/tool/Cargo.toml",
            "[package]\nname = 'tool'\n",
        ),
        ("scripts/rust/crates/tool/src/main.rs", "fn main() {}\n"),
        (
            "scripts/rust/crates/pair/Cargo.toml",
            "[package]\nname = 'pair'\n\n[[bin]]\nname = 'left'\npath = 'src/bin/left.rs'\n\n[[bin]]\nname = 'right'\npath = 'src/bin/right.rs'\n",
        ),
        (
            "scripts/rust/crates/library/Cargo.toml",
            "[package]\nname = 'library'\n",
        ),
        ("scripts/rust/crates/library/src/lib.rs", ""),
        (
            "scripts/python/pyproject.toml",
            "[project]\nname = 'tools'\n\n[project.scripts]\ntranscript = 'tools.cli:app'\n",
        ),
        ("scripts/python/uv.lock", ""),
        ("scripts/go/go.mod", "module dotfiles/tools\n"),
        ("scripts/go/cmd/copy-file-pretty/main.go", "package main\n"),
        (".bin/", ""),
    ])
}

fn context(root: &Path) -> Context {
    let mut context = Context::new(
        root.to_path_buf(),
        root.join("home"),
        root.join("config"),
        root.join("home/.config"),
    )
    .unwrap();
    // Enough PATH for the stubs to run, never enough to find a real toolchain.
    let path = format!("{}:/usr/bin:/bin", stubs(root).display());
    context.process_env.insert("PATH".into(), path.into());
    context
}

/// Stand-ins for cargo, go and uv that write the artefacts the real ones would.
fn stubs(root: &Path) -> PathBuf {
    let path = root.join("stubs");
    if path.is_dir() {
        return path;
    }
    fs::create_dir_all(&path).unwrap();
    let target = root.join("scripts/rust/target");
    executable(
        &path.join("cargo"),
        &format!(
            "#!/bin/sh\nprintf 'cargo\\n' >> \"$BUILD_LOG\"\n\
             [ -n \"$BUILD_FAILS\" ] && {{ echo 'cargo exploded' >&2; exit 1; }}\n\
             while [ $# -gt 0 ]; do case \"$1\" in\n\
               --profile) profile=$2; shift ;;\n\
               --package) case \"$2\" in pair) names=\"$names left right\" ;; *) names=\"$names $2\" ;; esac; shift ;;\n\
             esac; shift; done\n\
             mkdir -p '{target}'/$profile\n\
             for name in $names; do printf 'rust %s %s' \"$name\" \"$BUILD_TAG\" > '{target}'/$profile/$name; done\n",
            target = target.display()
        ),
    );
    executable(
        &path.join("go"),
        "#!/bin/sh\nprintf 'go\\n' >> \"$BUILD_LOG\"\n\
         while [ $# -gt 0 ]; do [ \"$1\" = -o ] && { out=$2; shift; }; shift; done\n\
         mkdir -p \"$out\"\nprintf 'go %s' \"$BUILD_TAG\" > \"$out/copy-file-pretty\"\n",
    );
    executable(
        &path.join("uv"),
        "#!/bin/sh\nprintf 'uv %s\\n' \"$1\" >> \"$BUILD_LOG\"\n\
         [ \"$1\" = export ] && exit 0\n\
         [ \"$1\" = tool ] || exit 0\n\
         mkdir -p \"$UV_TOOL_BIN_DIR\"\n\
         printf 'python %s' \"$BUILD_TAG\" > \"$UV_TOOL_BIN_DIR/transcript\"\n\
         chmod 0755 \"$UV_TOOL_BIN_DIR/transcript\"\n",
    );
    path
}

fn ensure(context: &Context, options: &install::Options) -> Result<install::Report, String> {
    install::ensure(context, options, &VecSink::default())
}

fn log(root: &Path) -> Vec<String> {
    fs::read_to_string(root.join("build.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn with_build_env(context: &mut Context, root: &Path, tag: &str) {
    context
        .process_env
        .insert("BUILD_LOG".into(), root.join("build.log").into());
    context.process_env.insert("BUILD_TAG".into(), tag.into());
}

#[test]
fn binaries_are_derived_from_the_manifests_that_build_them() {
    let root = sandbox();
    let toolchain = Toolchain::read(root.path()).unwrap();

    assert_eq!(
        toolchain.stage(Language::Rust).unwrap().binaries,
        ["left", "right", "tool"],
        "explicit [[bin]] targets and an implicit src/main.rs, never a library"
    );
    assert_eq!(
        toolchain.stage(Language::Python).unwrap().binaries,
        ["transcript"]
    );
    assert_eq!(
        toolchain.stage(Language::Go).unwrap().binaries,
        ["copy-file-pretty"]
    );
}

#[test]
fn crates_that_opt_into_release_build_beside_the_commands_profile() {
    let root = sandbox();
    fs::write(
        root.path().join("scripts/rust/Cargo.toml"),
        "[workspace]\nmembers = ['crates/tool', 'crates/pair', 'crates/library', 'crates/probe']\n",
    )
    .unwrap();
    fs::create_dir_all(root.path().join("scripts/rust/crates/probe/src")).unwrap();
    fs::write(
        root.path().join("scripts/rust/crates/probe/Cargo.toml"),
        "[package]\nname = 'probe'\n\n[package.metadata.dotfile]\nprofile = 'release'\n",
    )
    .unwrap();
    fs::write(
        root.path().join("scripts/rust/crates/probe/src/main.rs"),
        "fn main() {}\n",
    )
    .unwrap();
    let toolchain = Toolchain::read(root.path()).unwrap();
    let profiles: Vec<(&str, &str)> = toolchain
        .stage(Language::Rust)
        .unwrap()
        .crates
        .iter()
        .map(|krate| (krate.package.as_str(), krate.profile.as_str()))
        .collect();
    assert_eq!(
        profiles,
        [
            ("tool", "commands"),
            ("pair", "commands"),
            ("probe", "release")
        ]
    );
    let mut context = context(root.path());
    with_build_env(&mut context, root.path(), "first");

    ensure(&context, &install::Options::native()).unwrap();

    let cargo = log(root.path())
        .iter()
        .filter(|line| *line == "cargo")
        .count();
    assert_eq!(cargo, 2, "one Cargo invocation per profile");
    let target = root.path().join("scripts/rust/target");
    assert!(target.join("release/probe").is_file());
    assert!(!target.join("commands/probe").exists());
    assert!(target.join("commands/tool").is_file());
    for name in ["tool", "left", "right", "probe"] {
        assert_eq!(
            fs::read_to_string(root.path().join(".bin").join(name)).unwrap(),
            format!("rust {name} first")
        );
    }
}

#[test]
fn every_language_installs_its_binaries_and_stamps_them() {
    let root = sandbox();
    let mut context = context(root.path());
    with_build_env(&mut context, root.path(), "first");

    let report = ensure(&context, &install::Options::everything()).unwrap();

    assert_eq!(report.installed.len(), 5);
    for name in ["tool", "left", "right", "transcript", "copy-file-pretty"] {
        let installed = root.path().join(".bin").join(name);
        assert!(installed.is_file(), "{name} was not installed");
        assert_ne!(
            fs::metadata(&installed).unwrap().permissions().mode() & 0o111,
            0,
            "{name} was installed without its exec bit"
        );
    }
    for language in ["rust", "python", "go"] {
        assert!(
            root.path().join("config/sync").join(language).is_file(),
            "{language} was not stamped"
        );
    }
}

#[test]
fn an_unchanged_toolchain_is_not_rebuilt() {
    let root = sandbox();
    let mut context = context(root.path());
    with_build_env(&mut context, root.path(), "first");
    ensure(&context, &install::Options::everything()).unwrap();
    fs::remove_file(root.path().join("build.log")).unwrap();

    let report = ensure(&context, &install::Options::everything()).unwrap();

    assert!(report.is_empty());
    assert!(log(root.path()).is_empty(), "no builder was invoked");
}

#[test]
fn a_changed_source_rebuilds_only_its_own_language() {
    let root = sandbox();
    let mut context = context(root.path());
    with_build_env(&mut context, root.path(), "first");
    ensure(&context, &install::Options::everything()).unwrap();
    fs::remove_file(root.path().join("build.log")).unwrap();
    fs::write(
        root.path().join("scripts/go/cmd/copy-file-pretty/main.go"),
        "package main // changed\n",
    )
    .unwrap();
    with_build_env(&mut context, root.path(), "second");

    let report = ensure(&context, &install::Options::everything()).unwrap();

    assert_eq!(report.rebuilt, [Language::Go]);
    assert_eq!(log(root.path()), ["go"]);
    assert_eq!(
        fs::read_to_string(root.path().join(".bin/copy-file-pretty")).unwrap(),
        "go second"
    );
    assert_eq!(
        fs::read_to_string(root.path().join(".bin/tool")).unwrap(),
        "rust tool first",
        "an untouched language keeps the binary it already had"
    );
}

#[test]
fn native_only_never_reaches_the_python_toolchain() {
    let root = sandbox();
    let mut context = context(root.path());
    with_build_env(&mut context, root.path(), "first");

    ensure(&context, &install::Options::native()).unwrap();

    let mut builders = log(root.path());
    builders.sort();
    assert_eq!(builders, ["cargo", "go"], "languages build in parallel");
    assert!(!root.path().join(".bin/transcript").exists());
    assert!(!root.path().join("config/sync/python").exists());
}

#[test]
fn a_failed_build_leaves_the_installed_tools_alone() {
    let root = sandbox();
    let mut context = context(root.path());
    with_build_env(&mut context, root.path(), "first");
    ensure(&context, &install::Options::everything()).unwrap();
    fs::write(
        root.path().join("scripts/rust/crates/tool/src/main.rs"),
        "fn main() { /* changed */ }\n",
    )
    .unwrap();
    context.process_env.insert("BUILD_FAILS".into(), "1".into());
    with_build_env(&mut context, root.path(), "second");

    let error = ensure(&context, &install::Options::everything()).unwrap_err();

    assert!(error.contains("cargo exploded"), "{error}");
    assert_eq!(
        fs::read_to_string(root.path().join(".bin/tool")).unwrap(),
        "rust tool first",
        "the previously installed binary survives a failed rebuild"
    );
    assert_eq!(
        digest::of(&[root.path().join("scripts/go")]).unwrap(),
        fs::read_to_string(root.path().join("config/sync/go"))
            .unwrap()
            .trim(),
        "stamps stay consistent with what is installed"
    );
}

#[test]
fn a_missing_go_toolchain_costs_only_the_go_commands() {
    let root = sandbox();
    let mut context = context(root.path());
    with_build_env(&mut context, root.path(), "first");
    fs::remove_file(root.path().join("stubs/go")).unwrap();

    let report = ensure(&context, &install::Options::everything()).unwrap();

    assert!(!report.rebuilt.contains(&Language::Go));
    assert!(root.path().join(".bin/tool").is_file());
    assert!(!root.path().join(".bin/copy-file-pretty").exists());
}

#[test]
fn a_missing_cargo_is_fatal() {
    let root = sandbox();
    let mut context = context(root.path());
    with_build_env(&mut context, root.path(), "first");
    fs::remove_file(root.path().join("stubs/cargo")).unwrap();

    let error = ensure(&context, &install::Options::everything()).unwrap_err();

    assert!(error.contains("cargo is required"), "{error}");
}

#[test]
fn retired_commands_are_removed_and_completions_written() {
    let root = sandbox();
    let mut context = context(root.path());
    with_build_env(&mut context, root.path(), "first");
    let retired = root.path().join(".bin/tardirs");
    let completion = root.path().join(".cache/zsh/tardirs-completion.zsh");
    fs::create_dir_all(completion.parent().unwrap()).unwrap();
    fs::write(&retired, "old").unwrap();
    fs::write(&completion, "old").unwrap();

    let report = ensure(&context, &install::Options::everything()).unwrap();

    assert_eq!(report.pruned, 2);
    assert!(!retired.exists());
    assert!(!completion.exists());
    assert!(
        root.path()
            .join(".cache/zsh/tools-completion.zsh")
            .is_file()
    );
}

#[test]
fn a_digest_follows_content_and_ignores_build_output() {
    let root = sandbox();
    let inputs = [root.path().join("scripts/go")];
    let before = digest::of(&inputs).unwrap();

    fs::create_dir_all(root.path().join("scripts/go/target")).unwrap();
    fs::write(root.path().join("scripts/go/target/junk"), "ignored").unwrap();
    assert_eq!(digest::of(&inputs).unwrap(), before);

    fs::write(root.path().join("scripts/go/go.mod"), "module other\n").unwrap();
    assert_ne!(digest::of(&inputs).unwrap(), before);
}

#[test]
fn a_cached_digest_holds_until_an_input_moves() {
    let root = sandbox();
    let inputs = [root.path().join("scripts/go")];
    let cache = root.path().join("config/sync/go.inputs");
    assert_eq!(
        digest::cached(&inputs, &cache).unwrap(),
        digest::of(&inputs).unwrap()
    );
    let saved = fs::read_to_string(&cache).unwrap();
    let (fingerprint, _) = saved.trim().split_once(' ').unwrap();
    fs::write(&cache, format!("{fingerprint} remembered\n")).unwrap();

    assert_eq!(
        digest::cached(&inputs, &cache).unwrap(),
        "remembered",
        "unchanged metadata never reads the sources"
    );

    fs::write(root.path().join("scripts/go/go.mod"), "module other\n").unwrap();
    assert_eq!(
        digest::cached(&inputs, &cache).unwrap(),
        digest::of(&inputs).unwrap()
    );
}
