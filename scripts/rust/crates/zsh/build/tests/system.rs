#![forbid(unsafe_code)]
#![cfg(unix)]

use std::fs;
use std::path::Path;

use testkit::{Bin, Ran, executable, tree_pairs};

const ENVTOOL: &str =
    "#!/bin/sh\necho envtool >> \"$PROBE_LOG\"\nprintf 'export TOOL_HOME=/tool\\ntool_seen=1\\n'\n";

/// zsh's startup order, with the global files in `sys` and `$0` naming the shell.
const DRIVER: &str = r#"setopt no_function_argzero
source zsh/env.zsh
if [[ -o login ]]; then
  [[ -o global_rcs && -r sys/zprofile ]] && source sys/zprofile
  [[ -r $HOME/.zprofile ]] && source $HOME/.zprofile
fi
if [[ -o interactive ]]; then
  [[ -o global_rcs && -r sys/zshrc ]] && source sys/zshrc
  setopt function_argzero
  source zsh/rc.zsh
fi
print -r -- "order=$order late=${late-unset} tool=${TOOL_HOME-unset} flag=${_zsh_build_rcs-unset} global=$options[globalrcs] path=$PATH man=${MANPATH-unset}""#;

fn zprofile() -> String {
    let path_helper = if cfg!(target_os = "macos") {
        "eval `path_helper -s`\n"
    } else {
        ""
    };
    format!("order+=(zprofile:$0)\n[[ -n $STOP ]] && return\nlate=1\n{path_helper}")
}

fn fixture() -> tempfile::TempDir {
    let root = tree_pairs(&[
        (
            "zsh/env.zsh",
            "export CONF=\"$HOME/zsh\"\n# zsh-build: omit\n[[ -o interactive && -o global_rcs && -z $NO_BUNDLE && -r $HOME/.cache/build/zshrc.rcs.zsh ]] && source $HOME/.cache/build/zshrc.rcs.zsh\n",
        ),
        (
            "zsh/rc.zsh",
            "# zsh-build: omit\nif [[ -z $NO_BUNDLE && -r $HOME/.cache/build/zshrc.zsh ]]; then\n  source $HOME/.cache/build/zshrc.zsh\n  return\nfi\norder+=(rc)\n",
        ),
        (
            "zsh/profile.zsh",
            "# zsh-build: omit\n(( ${+_zsh_build_rcs} )) && return\norder+=(profile)\n[[ -x $HOME/bin/envtool ]] && eval \"$($HOME/bin/envtool shellenv)\"\n",
        ),
        ("sys/zshrc", "order+=(zshrc)\nsetopt combining_chars\n"),
        ("ph/etc/paths", "/usr/bin\n/bin\n/ph/bin\n"),
        ("ph/etc/paths.d/10-x", "/x/bin\n/usr/bin\n"),
        ("ph/etc/manpaths", "/usr/share/man\n"),
    ]);
    let path = root.path();
    fs::write(path.join("sys/zprofile"), zprofile()).unwrap();
    let config = format!(
        "output = \".cache/build\"\nambient = [\"HOME\"]\n\n[[target]]\nname = \"zshrc\"\nsource = \"zsh/rc.zsh\"\nenv = [\"zsh/env.zsh\"]\nprofile = \"zsh/profile.zsh\"\nsystem = true\n\n[system]\ndir = \"{sys}\"\nambient = [\"HOME\"]\npath_helper_root = \"{ph}\"\n\n[fold]\ncommands = [\"envtool shellenv\"]\n",
        sys = path.join("sys").display(),
        ph = path.join("ph").display(),
    );
    fs::create_dir_all(path.join("config/zsh")).unwrap();
    fs::write(path.join("config/zsh/build.toml"), config).unwrap();
    fs::create_dir_all(path.join("bin")).unwrap();
    executable(&path.join("bin/envtool"), ENVTOOL);
    executable(
        &path.join("bin/path_helper"),
        &format!(
            "#!/bin/sh\nPATH_HELPER_ROOT={} exec /usr/libexec/path_helper \"$@\"\n",
            path.join("ph").display()
        ),
    );
    std::os::unix::fs::symlink(path.join("zsh/profile.zsh"), path.join(".zprofile")).unwrap();
    root
}

fn search_path(root: &Path) -> String {
    format!("{}:/usr/bin:/bin", root.join("bin").display())
}

fn build(root: &Path) -> Ran {
    Bin::new(env!("CARGO_BIN_EXE_zsh-build"))
        .arg("--root")
        .arg(root)
        .env("HOME", root)
        .env("PATH", search_path(root))
        .env("PROBE_LOG", root.join("build.log"))
        .env_remove("ZDOTDIR")
        .run()
}

fn startup(root: &Path, flags: &str, bundle: bool, extra: &[(&str, &str)]) -> Ran {
    let mut command = Bin::new("zsh")
        .args([flags, DRIVER])
        .current_dir(root)
        .env("HOME", root)
        .env("PATH", search_path(root))
        .env("PROBE_LOG", root.join("run.log"))
        .env_remove("ZDOTDIR")
        .env_remove("MANPATH");
    if !bundle {
        command = command.env("NO_BUNDLE", "1");
    }
    for (name, value) in extra {
        command = command.env(name, value);
    }
    command.run()
}

fn same(root: &Path, flags: &str, extra: &[(&str, &str)]) -> String {
    let sources = startup(root, flags, false, extra);
    let bundle = startup(root, flags, true, extra);
    assert!(
        sources.success() && bundle.success(),
        "{sources:?} {bundle:?}"
    );
    assert_eq!(bundle.stdout, sources.stdout, "{flags}");
    bundle.stdout
}

fn guard(root: &Path) -> std::path::PathBuf {
    root.join(".cache/build/zshrc.rcs.zsh")
}

/// Waits out the background `zcompile`, so a rebuild is newer than its wordcode.
fn settle_wordcode(root: &Path) {
    let wordcode = root.join(".cache/build/zshrc.zsh.zwc");
    for _ in 0..50 {
        if wordcode.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    std::thread::sleep(std::time::Duration::from_millis(1100));
}

fn bundle_text(root: &Path) -> String {
    fs::read_to_string(root.join(".cache/build/zshrc.zsh")).unwrap()
}

#[test]
fn login_and_interactive_shells_run_the_global_files_in_order() {
    let root = fixture();
    let built = build(root.path());
    assert!(built.success() && built.stderr.is_empty(), "{built:?}");
    assert!(guard(root.path()).exists());

    let login = same(root.path(), "-filc", &[]);
    assert!(
        login
            .contains("order=zprofile:zsh profile zshrc rc late=1 tool=/tool flag=unset global=on"),
        "{login}"
    );
    let interactive = same(root.path(), "-fic", &[]);
    assert!(
        interactive.contains("order=zshrc rc late=unset tool=unset flag=unset global=on"),
        "{interactive}"
    );
    let script = same(root.path(), "-flc", &[]);
    assert!(
        script.contains("order=zprofile:zsh profile late=1 tool=/tool"),
        "{script}"
    );
    same(root.path(), "-fc", &[]);
}

#[test]
fn a_return_in_a_global_file_leaves_only_that_file() {
    let root = fixture();
    assert!(build(root.path()).success());
    let login = same(root.path(), "-filc", &[("STOP", "1")]);
    assert!(
        login.contains("order=zprofile:zsh profile zshrc rc late=unset"),
        "{login}"
    );
}

#[test]
fn folded_profile_commands_do_not_run_at_startup() {
    let root = fixture();
    assert!(build(root.path()).success());
    startup(root.path(), "-filc", true, &[]);
    assert_eq!(
        fs::read_to_string(root.path().join("run.log")).unwrap_or_default(),
        ""
    );
    startup(root.path(), "-flc", true, &[]);
    assert_eq!(
        fs::read_to_string(root.path().join("run.log")).unwrap(),
        "envtool\n"
    );
}

#[test]
fn an_uncompilable_global_file_keeps_every_global_file_with_zsh() {
    let root = fixture();
    assert!(build(root.path()).success());
    assert!(guard(root.path()).exists());
    fs::write(
        root.path().join("sys/zprofile"),
        "emulate sh -c true\norder+=(zprofile)\n",
    )
    .unwrap();
    let built = build(root.path());
    assert!(built.success(), "{built:?}");
    assert!(
        built
            .stderr
            .contains("emulate is not modeled; global startup files left to zsh"),
        "{}",
        built.stderr
    );
    assert!(!guard(root.path()).exists());
    assert!(!bundle_text(root.path()).contains("_zsh_build_rcs"));
    let login = same(root.path(), "-filc", &[]);
    assert!(login.contains("order=zprofile profile zshrc rc"), "{login}");
}

#[test]
fn a_profile_that_is_not_the_linked_one_keeps_the_global_files_with_zsh() {
    let root = fixture();
    fs::remove_file(root.path().join(".zprofile")).unwrap();
    fs::write(root.path().join(".zprofile"), "order+=(other)\n").unwrap();
    let built = build(root.path());
    assert!(
        built.stderr.contains("is not linked to"),
        "{}",
        built.stderr
    );
    assert!(!guard(root.path()).exists());
    let login = same(root.path(), "-filc", &[]);
    assert!(
        login.contains("order=zprofile:zsh other zshrc rc"),
        "{login}"
    );
}

#[test]
fn a_global_file_changed_since_the_build_is_read_by_zsh_again() {
    let root = fixture();
    assert!(build(root.path()).success());
    let earlier = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
    fs::File::options()
        .write(true)
        .open(guard(root.path()))
        .unwrap()
        .set_modified(earlier)
        .unwrap();
    fs::write(root.path().join("sys/zshrc"), "order+=(zshrc2)\n").unwrap();
    let login = same(root.path(), "-filc", &[]);
    assert!(login.contains("zshrc2 rc"), "{login}");
    settle_wordcode(root.path());
    assert!(build(root.path()).stdout.contains("updated"));
    let rebuilt = startup(root.path(), "-filc", true, &[]);
    assert!(rebuilt.stdout.contains("zshrc2 rc"), "{rebuilt:?}");
    assert!(bundle_text(root.path()).contains("order+=(zshrc2)"));
}

#[cfg(target_os = "macos")]
#[test]
fn the_path_helper_emulation_matches_path_helper() {
    let data = tree_pairs(&[
        ("etc/paths", "/usr/bin\n/bin\n/usr/bin\n\n/p/bin"),
        ("etc/paths.d/10-b", "/b\n"),
        ("etc/paths.d/9-a", "/a\n/usr/bin\n"),
        ("etc/paths.d/.hidden", "/hidden\n"),
        ("etc/manpaths", "/usr/share/man\n"),
        ("etc/manpaths.d/x", "/m\n"),
    ]);
    let wrapper = data.path().join("path_helper");
    executable(
        &wrapper,
        &format!(
            "#!/bin/sh\necho run >> {log}\nPATH_HELPER_ROOT={root} exec /usr/libexec/path_helper \"$@\"\n",
            log = data.path().join("runs").display(),
            root = data.path().display()
        ),
    );
    let runs = || {
        fs::read_to_string(data.path().join("runs"))
            .unwrap_or_default()
            .len()
    };
    let roots = [
        (
            data.path().to_str().unwrap().to_string(),
            wrapper.display().to_string(),
        ),
        (String::new(), "/usr/libexec/path_helper".to_string()),
    ];
    let cases: &[(Option<&str>, Option<&str>)] = &[
        (Some(""), None),
        (Some("/usr/bin:/x:/x::/b:"), None),
        (Some("/x"), Some("")),
        (Some("/x"), Some("/m2:/usr/share/man::/m2")),
        (Some("/a b:/c:/a b"), None),
        (Some("/a  b"), None),
        (Some("/a$HOME:/q\"x"), Some("/m`x`")),
        (Some("/a\tb"), None),
        (None, None),
    ];
    let report = r#"print -r -- "$PATH|${(t)PATH}|${MANPATH-unset}|${(t)MANPATH}""#;
    for (root, command) in &roots {
        let helper = zsh_build::native::PathHelper::load(root).unwrap();
        let original = format!("eval `{command} -s`");
        let emulated = helper.code(&original);
        for (index, (path, manpath)) in cases.iter().enumerate() {
            let run = |script: String| {
                let mut zsh = Bin::new("/bin/zsh")
                    .args(["-fc", &format!("{script}\n{report}")])
                    .env_remove("PATH_HELPER_ROOT")
                    .env_remove("MANPATH")
                    .env_remove("PATH");
                if let Some(path) = path {
                    zsh = zsh.env("PATH", path);
                }
                if let Some(manpath) = manpath {
                    zsh = zsh.env("MANPATH", manpath);
                }
                zsh.run()
            };
            let real = run(original.clone());
            let before = runs();
            let native = run(emulated.clone());
            if index < 5 {
                assert_eq!(runs(), before, "{path:?} fell back to path_helper");
            }
            assert!(real.success() && native.success(), "{real:?} {native:?}");
            assert_eq!(native.stdout, real.stdout, "{root:?} {path:?} {manpath:?}");
        }
    }
}
