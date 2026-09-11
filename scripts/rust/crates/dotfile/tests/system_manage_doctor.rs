#![cfg(unix)]
#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use testkit::{Bin, TempDir, executable};

struct Fixture {
    temporary: TempDir,
    root: PathBuf,
    home: PathBuf,
    bin: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("repo");
        let home = temporary.path().join("home");
        let bin = temporary.path().join("bin");
        for path in [
            root.join("config"),
            root.join("environment/test"),
            root.join("shared"),
            home.join(".config/dotfile"),
            bin.clone(),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(root.join("config/targets.dotfile"), "").unwrap();
        fs::write(root.join("environment/test/manifest"), "shared\n").unwrap();
        fs::write(home.join(".config/dotfile/profile"), "test\n").unwrap();
        assert!(
            Command::new("git")
                .arg("init")
                .arg("-q")
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        Self {
            temporary,
            root,
            home,
            bin,
        }
    }
    fn write(&self, relative: &str, value: &str) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }
    fn command(&self) -> Bin {
        Bin::new(env!("CARGO_BIN_EXE_dotfile"))
            .env("DOTFILE_ROOT", &self.root)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("SHELL", "/bin/zsh")
            .env_remove("USER")
            .env_remove("SYSINFO_HOST")
            .env_remove("SYSINFO_CONFIG")
            .env(
                "SYSINFO_BENCHMARKS",
                self.temporary.path().join("benchmarks"),
            )
            .current_dir(&self.root)
            .plain()
    }
    fn system(&self, text: &str) -> PathBuf {
        let target = self.temporary.path().join("etc");
        fs::create_dir_all(&target).unwrap();
        self.write("shared/service/.system", "");
        self.write("shared/service/etc/service.conf", text);
        self.write(
            "config/targets.dotfile",
            &format!("shared/service/etc = {}\n", target.display()),
        );
        target
    }
}

#[test]
fn system_dry_run_does_not_write_destinations_or_plaintext_staging_files() {
    let fixture = Fixture::new();
    let target = fixture.system("setting=true\n");
    let temp = fixture.temporary.path().join("private-tmp");
    fs::create_dir(&temp).unwrap();
    let result = fixture
        .command()
        .args(["system", "install", "--dry-run"])
        .env("TMPDIR", &temp)
        .run();
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    assert!(result.stdout.contains("sudo install -D -o root -g root"));
    assert!(!target.join("service.conf").exists());
    assert_eq!(fs::read_dir(temp).unwrap().count(), 0);
    assert!(!fixture.home.join(".config/dotfile/mutation.lock").exists());
}

#[test]
fn system_preflight_refuses_all_writes_when_one_template_is_unresolved() {
    let fixture = Fixture::new();
    let target = fixture.system("setting=true\n");
    fixture.write(
        "shared/service/etc/private.conf.tmpl",
        "password={{ absent }}\n",
    );
    let result = fixture.command().args(["system", "install", "-n"]).run();
    assert_eq!(result.code(), Some(1));
    assert!(result.stdout.contains("unresolved"), "{}", result.stdout);
    assert!(!target.join("service.conf").exists());
}

#[test]
fn system_private_diff_redacts_both_versions() {
    let fixture = Fixture::new();
    let target = fixture.system("setting=true\n");
    fixture.write("shared/service/etc/private.conf.tmpl", "secret-new-value\n");
    fs::write(target.join("private.conf"), "secret-old-value\n").unwrap();
    let result = fixture
        .command()
        .args(["system", "diff", "private.conf"])
        .run();
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    assert!(result.stdout.contains("private rendered content differs"));
    assert!(!result.stdout.contains("secret-new-value"));
    assert!(!result.stdout.contains("secret-old-value"));
}

#[test]
fn system_rejects_home_targets_and_symlink_aliases() {
    let fixture = Fixture::new();
    fixture.system("setting=true\n");
    let alias = fixture.temporary.path().join("alias");
    std::os::unix::fs::symlink(&fixture.home, &alias).unwrap();
    fixture.write(
        "config/targets.dotfile",
        &format!("shared/service/etc = {}\n", alias.display()),
    );
    let result = fixture.command().args(["system", "status"]).run();
    assert_eq!(result.code(), Some(1));
    assert!(result.stdout.contains("under $HOME"));
}

#[test]
fn system_add_validates_repository_components_before_copying() {
    let fixture = Fixture::new();
    let target = fixture.system("setting=true\n");
    let source = target.join("outside");
    fs::write(&source, "outside").unwrap();
    let result = fixture
        .command()
        .args(["system", "add"])
        .arg(&source)
        .args(["--pkg", "../../escape", "--group", "shared"])
        .run();
    assert_eq!(result.code(), Some(1));
    assert_eq!(fs::read_to_string(source).unwrap(), "outside");
    assert!(!fixture.temporary.path().join("escape").exists());
}

#[test]
fn add_then_remove_preserves_content_and_stages_only_relevant_paths() {
    let fixture = Fixture::new();
    let source = fixture.home.join(".config/widget/config.toml");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, "a=1\n").unwrap();
    fixture.write("unrelated", "not staged");
    let added = fixture
        .command()
        .arg("add")
        .arg(&source)
        .args(["--description", "Widget settings"])
        .run();
    assert_eq!(added.code(), Some(0), "{}", added.stderr);
    assert!(source.is_symlink());
    assert_eq!(fs::read_to_string(&source).unwrap(), "a=1\n");
    assert!(
        fs::read_to_string(fixture.root.join("config/packages.dotfile"))
            .unwrap()
            .contains("Widget settings")
    );
    let removed = fixture.command().args(["remove", "shared/widget"]).run();
    assert_eq!(removed.code(), Some(0), "{}", removed.stderr);
    assert!(!source.is_symlink());
    assert_eq!(fs::read_to_string(&source).unwrap(), "a=1\n");
    assert!(!fixture.root.join("shared/widget").exists());
    let output = Command::new("git")
        .arg("-C")
        .arg(&fixture.root)
        .args(["diff", "--cached", "--name-only"])
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|path| path == "unrelated")
    );
}

#[test]
fn add_rollback_restores_original_config_and_git_index_on_staging_failure() {
    let fixture = Fixture::new();
    let source = fixture.home.join(".config/widget.conf");
    fs::write(&source, "original\n").unwrap();
    fixture.write("kept", "already staged");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&fixture.root)
            .args(["add", "kept"])
            .status()
            .unwrap()
            .success()
    );
    let index = fs::read(fixture.root.join(".git/index")).unwrap();
    executable(
        &fixture.bin.join("git"),
        "#!/bin/sh\nif [ \"$3\" = rev-parse ]; then printf '.git/index\\n'; exit 0; fi\nprintf 'failed deliberately\\n' >&2\nexit 72\n",
    );
    let result = fixture.command().arg("add").arg(&source).run();
    assert_eq!(result.code(), Some(1));
    assert!(result.stderr.contains("staging failed"));
    assert!(!source.is_symlink());
    assert_eq!(fs::read_to_string(&source).unwrap(), "original\n");
    assert!(!fixture.root.join("shared/widget").exists());
    assert_eq!(fs::read(fixture.root.join(".git/index")).unwrap(), index);
    assert_eq!(
        fs::read_to_string(fixture.root.join("config/targets.dotfile")).unwrap(),
        ""
    );
    assert!(!fixture.root.join("config/packages.dotfile").exists());
}

#[test]
fn remove_preserves_foreign_live_edits_and_unfolds_managed_parent() {
    let fixture = Fixture::new();
    fixture.write("shared/widget/a", "repo-a");
    fixture.write("shared/widget/b", "repo-b");
    let live = fixture.home.join(".config/widget");
    std::os::unix::fs::symlink(fixture.root.join("shared/widget"), &live).unwrap();
    let result = fixture.command().args(["remove", "shared/widget/a"]).run();
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    assert!(live.is_dir());
    assert!(!live.is_symlink());
    assert!(!live.join("a").is_symlink());
    assert!(live.join("b").is_symlink());
    assert_eq!(fs::read_to_string(live.join("a")).unwrap(), "repo-a");
    fs::remove_file(live.join("b")).unwrap();
    fs::write(live.join("b"), "live-edit").unwrap();
    let result = fixture.command().args(["remove", "shared/widget/b"]).run();
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    assert_eq!(fs::read_to_string(live.join("b")).unwrap(), "live-edit");
}

#[test]
fn doctor_reports_missing_links_requirements_and_version_pins_without_writes() {
    let fixture = Fixture::new();
    fixture.write("shared/widget/config", "repo");
    fixture.write(
        "config/requirements.dotfile",
        "shared {\n missing-doctor-test = install-me\n}\n",
    );
    fixture.write(
        "config/pins.dotfile",
        "shared {\n pinned-test = expected-version\n}\n",
    );
    executable(
        &fixture.bin.join("pinned-test"),
        "#!/bin/sh\nprintf 'old-version\\n'\n",
    );
    let result = fixture.command().args(["doctor", "--all"]).run();
    assert_eq!(result.code(), Some(1), "{}", result.stderr);
    for text in [
        "links",
        "tools",
        "pins",
        "missing-doctor-test",
        "expected-version",
    ] {
        assert!(
            result.stdout.contains(text),
            "missing {text}: {}",
            result.stdout
        );
    }
    assert!(!fixture.home.join(".config/widget").exists());
}

#[test]
fn doctor_caches_package_inventory_and_preserves_section_order() {
    let fixture = Fixture::new();
    fixture.write("environment/test/pkglist.txt", "one\n");
    fixture.write("environment/test/aurlist.txt", "two\n");
    let log = fixture.temporary.path().join("pacman-calls");
    executable(
        &fixture.bin.join("pacman"),
        "#!/bin/sh\nprintf 'call\\n' >> \"$PROBE_LOG\"\nprintf 'one\\ntwo\\n'\n",
    );
    let result = fixture
        .command()
        .args(["doctor"])
        .env("PROBE_LOG", &log)
        .run();
    assert_eq!(
        result.code(),
        Some(0),
        "{}\n{}",
        result.stdout,
        result.stderr
    );
    assert_eq!(fs::read_to_string(log).unwrap(), "call\n");
    assert!(result.stdout.find("links").unwrap() < result.stdout.find("pkglist").unwrap());
    assert!(result.stdout.find("pkglist").unwrap() < result.stdout.find("aurlist").unwrap());
}

#[test]
fn doctor_reads_saved_benchmark_host_without_python() {
    let fixture = Fixture::new();
    fs::write(fixture.home.join(".config/dotfile/host"), "fixture-host\n").unwrap();
    let directory = fixture.temporary.path().join("benchmarks/fixture-host");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("run.json"),
        r#"{"grade":"clean","started":"2000-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    let result = fixture.command().arg("doctor").run();
    assert_eq!(result.code(), Some(1));
    assert!(result.stdout.contains("last clean run was"));
}

#[test]
fn profile_relevance_uses_manifest_desktop_requirements() {
    let fixture = Fixture::new();
    fixture.write("environment/arch-linux/kde/manifest", "shared\nlinux/kde\n");
    fixture.write(
        "environment/arch-linux/both/manifest",
        "shared\nlinux/kde\nlinux/hyprland\n",
    );
    let context = dotfile_cli::context::Context::new(
        fixture.root.clone(),
        fixture.home.clone(),
        fixture.home.join(".config/dotfile"),
    )
    .unwrap();
    let profiles = dotfile_cli::config::profiles::filter(
        &context,
        "arch-linux",
        &[("linux/kde", true), ("linux/hyprland", false)],
    )
    .unwrap();
    assert_eq!(profiles, ["arch-linux/kde"]);
    assert_eq!(
        dotfile_cli::config::profiles::linux_platform("ID=cachyos\nID_LIKE=\"arch\"\n"),
        "arch-linux"
    );
}

#[test]
fn generated_metadata_symlinks_cannot_redirect_transaction_writes() {
    let fixture = Fixture::new();
    let source = fixture.home.join(".config/widget.conf");
    fs::write(&source, "original\n").unwrap();
    let victim = fixture.temporary.path().join("victim");
    fs::write(&victim, "do not change\n").unwrap();
    std::os::unix::fs::symlink(&victim, fixture.root.join("PACKAGES.md")).unwrap();
    let result = fixture.command().arg("add").arg(&source).run();
    assert_eq!(result.code(), Some(1));
    assert_eq!(fs::read_to_string(&victim).unwrap(), "do not change\n");
    assert_eq!(fs::read_to_string(&source).unwrap(), "original\n");
    assert!(!source.is_symlink());
    assert!(fixture.root.join("PACKAGES.md").is_symlink());
    assert!(!fixture.root.join("shared/widget").exists());
}

#[test]
fn system_status_detects_mode_and_owner_drift() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let fixture = Fixture::new();
    let target = fixture.system("setting=true\n").join("service.conf");
    fs::write(&target, "setting=true\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o666)).unwrap();
    let result = fixture.command().args(["system", "status"]).run();
    assert_eq!(result.code(), Some(1));
    assert!(result.stdout.contains("mode 0666, want 0644"));
    if fs::metadata(&target).unwrap().uid() != 0 {
        assert!(result.stdout.contains("want root:root"));
    }
}

#[test]
fn failed_remove_staging_restores_folded_link_and_repository() {
    let fixture = Fixture::new();
    fixture.write("shared/widget/a", "original");
    let live = fixture.home.join(".config/widget");
    std::os::unix::fs::symlink(fixture.root.join("shared/widget"), &live).unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&fixture.root)
            .args(["add", "shared/widget/a"])
            .status()
            .unwrap()
            .success()
    );
    let index = fs::read(fixture.root.join(".git/index")).unwrap();
    let real_git = Command::new("/bin/sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    executable(
        &fixture.bin.join("git"),
        "#!/bin/sh\ncase \" $* \" in *' add '*) printf 'denied\\n' >&2; exit 71;; esac\nexec \"$REAL_GIT\" \"$@\"\n",
    );
    let result = fixture
        .command()
        .args(["remove", "shared/widget/a"])
        .env(
            "REAL_GIT",
            String::from_utf8(real_git.stdout).unwrap().trim(),
        )
        .run();
    assert_eq!(result.code(), Some(1));
    assert!(live.is_symlink());
    assert_eq!(fs::read_to_string(live.join("a")).unwrap(), "original");
    assert_eq!(fs::read(fixture.root.join(".git/index")).unwrap(), index);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_root_install_applies_ownership_modes_and_is_idempotent() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if !Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|output| output.stdout == b"0\n")
    {
        eprintln!("actual privileged install requires the disposable root test container");
        return;
    }
    if !Command::new("sudo")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        eprintln!("actual privileged install requires sudo in the disposable test container");
        return;
    }
    let fixture = Fixture::new();
    let target = fixture.system("setting=true\n");
    fixture.write("shared/service/etc/private.conf.tmpl", "private-example\n");
    fixture.write("shared/service/etc/script", "#!/bin/sh\nexit 0\n");
    fs::set_permissions(
        fixture.root.join("shared/service/etc/script"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let result = fixture.command().args(["system", "install", "--yes"]).run();
    assert_eq!(
        result.code(),
        Some(0),
        "{}\n{}",
        result.stdout,
        result.stderr
    );
    for (name, mode) in [
        ("service.conf", 0o644),
        ("private.conf", 0o600),
        ("script", 0o755),
    ] {
        let metadata = fs::metadata(target.join(name)).unwrap();
        assert_eq!(metadata.mode() & 0o7777, mode);
        assert_eq!((metadata.uid(), metadata.gid()), (0, 0));
    }
    let modified = fs::metadata(target.join("service.conf"))
        .unwrap()
        .modified()
        .unwrap();
    let status = fixture.command().args(["system", "status"]).run();
    assert_eq!(status.code(), Some(0), "{}", status.stdout);
    assert!(status.stdout.contains("3 current"));
    let again = fixture.command().args(["system", "install", "--yes"]).run();
    assert_eq!(again.code(), Some(0));
    assert!(again.stdout.contains("nothing to install"));
    assert_eq!(
        fs::metadata(target.join("service.conf"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
}
