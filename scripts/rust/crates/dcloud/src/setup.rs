use crate::config::{self, Config, Destination, DestinationKind, HostConfig, Job};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub fn private_dir(path: &Path) -> Result<()> {
    ensure!(
        !fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()),
        "private directory is a symlink: {}",
        path.display()
    );
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn write_private_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("file has no parent")?;
    private_dir(parent)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn check_secret(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("recovery credential missing: {}", path.display()))?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "credential must be a regular file: {}",
        path.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "credential permissions must be 0600: {}",
            path.display()
        );
    }
    Ok(())
}

pub fn example(host: &str) -> Result<Config> {
    config::identifier(host)?;
    let mut c = Config {
        host: host.into(),
        ..Config::default()
    };
    c.state_dir = config::expand(&c.state_dir)?;
    c.password_file = config::expand(&c.password_file)?;
    c.identity_file = config::expand(&c.identity_file)?;
    c.hosts.insert(
        "archie".into(),
        HostConfig {
            ssh: Some("archie".into()),
            ..HostConfig::default()
        },
    );
    c.hosts.insert(
        "macie".into(),
        HostConfig {
            ssh: Some("macie".into()),
            ..HostConfig::default()
        },
    );
    c.destinations.insert(
        "drive".into(),
        Destination {
            kind: DestinationKind::Drive,
            location: "dcloud-drive:dcloud".into(),
            offsite: true,
            encrypted: false,
            ..Destination::default()
        },
    );
    c.destinations.insert(
        "vps".into(),
        Destination {
            kind: DestinationKind::Sftp,
            location: "backup-vps:/srv/dcloud".into(),
            offsite: true,
            ..Destination::default()
        },
    );
    c.jobs.insert(
        "Documents".into(),
        Job {
            sources: [
                ("archie".into(), vec![PathBuf::from("~/Documents")]),
                ("macie".into(), vec![PathBuf::from("~/Documents")]),
            ]
            .into(),
            destinations: vec!["drive".into()],
            required: vec!["drive".into()],
            category: "documents".into(),
            labels: vec!["personal".into()],
            ..Job::default()
        },
    );
    c.jobs.insert(
        "Pictures".into(),
        Job {
            sources: [("archie".into(), vec![PathBuf::from("~/Pictures")])].into(),
            destinations: vec!["drive".into(), "vps".into()],
            required: vec!["drive".into(), "vps".into()],
            min_copies: 2,
            require_offsite: true,
            category: "photos".into(),
            ..Job::default()
        },
    );
    c.validate()?;
    Ok(c)
}

pub fn initialize(path: &Path, host: &str, secrets_dir: Option<&Path>) -> Result<Value> {
    ensure!(
        !path.exists(),
        "configuration already exists: {}",
        path.display()
    );
    let mut config = example(host)?;
    let key_dir = path.parent().context("config has no parent")?;
    config.password_file = key_dir.join("repository.key");
    config.identity_file = key_dir.join("identity.txt");
    if let Some(directory) = secrets_dir {
        let directory = config::expand(directory)?;
        let directory = if directory.is_absolute() {
            directory
        } else {
            std::env::current_dir()?.join(directory)
        };
        config.secrets_file = Some(directory.join("recovery.sops.json"));
        config.rclone_secrets_file = Some(directory.join("rclone.sops.json"));
        config.recipients = vec![crate::secrets::initialize(&config)?];
        write_private_new(path, toml::to_string_pretty(&config)?.as_bytes())?;
        return Ok(
            json!({"config":path,"secrets":config.secrets_file,"recipient":config.recipients[0],"next":"import encrypted Google client, authorize Drive, run doctor --remote"}),
        );
    }
    if !config.password_file.exists() {
        let password = format!(
            "{}{}\n",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        write_private_new(&config.password_file, password.as_bytes())?;
    }
    if !config.identity_file.exists() {
        let (identity, _) = crate::archive::generate_identity();
        write_private_new(&config.identity_file, format!("{identity}\n").as_bytes())?;
    }
    check_secret(&config.identity_file)?;
    check_secret(&config.password_file)?;
    let key = fs::read_to_string(&config.identity_file)?;
    let identity = key
        .lines()
        .find(|v| v.starts_with("AGE-SECRET-KEY-"))
        .context("identity file has no X25519 identity")?
        .parse::<age::x25519::Identity>()
        .map_err(anyhow::Error::msg)?;
    config.recipients = vec![identity.to_public().to_string()];
    write_private_new(path, toml::to_string_pretty(&config)?.as_bytes())?;
    private_dir(&config.state_dir)?;
    Ok(
        json!({"config":path,"host":host,"recipient":config.recipients[0],"next":"edit destinations, authorize Drive, run doctor --remote"}),
    )
}

pub fn execute(
    command: &mut Command,
    timeout: Duration,
) -> Result<hostkit::process::CapturedOutput> {
    let result = hostkit::process::output(
        command,
        hostkit::process::CaptureLimits {
            stdout: 8 * 1024 * 1024,
            stderr: 64 * 1024,
        },
        timeout,
    )?;
    ensure!(
        result.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&result.stderr).trim()
    );
    ensure!(!result.stdout_truncated, "command output limit exceeded");
    Ok(result)
}

pub fn doctor(config: &Config, remote: bool) -> Result<Value> {
    let mut checks = Vec::new();
    let mut errors = Vec::new();
    for (name, path, args) in [
        ("restic", &config.tools.restic, vec!["version"]),
        ("rclone", &config.tools.rclone, vec!["version"]),
    ] {
        match execute(Command::new(path).args(args),Duration::from_secs(15)) {Ok(output)=>checks.push(json!({"check":name,"version":String::from_utf8_lossy(&output.stdout).lines().next().unwrap_or("")})),Err(e)=>errors.push(format!("{name}: {e:#}"))}
    }
    for path in [&config.password_file, &config.identity_file] {
        match check_secret(path) {
            Ok(()) => checks.push(json!({"credential":path,"permissions":"private"})),
            Err(e) => errors.push(format!("{e:#}")),
        }
    }
    for (name, job) in &config.jobs {
        if let Some(paths) = job.sources.get(&config.host) {
            for path in paths {
                let path = config::expand(path)?;
                let access = source_access(&path);
                checks.push(json!({"job":name,"source":path,"exists":path.exists(),"readable":access.is_ok()}));
                if let Err(error) = access {
                    errors.push(format!("{error:#}"));
                }
            }
        }
    }
    if remote {
        for (name, dest) in &config.destinations {
            let result = (|| -> Result<Value> {
                match dest.kind {
                    DestinationKind::Local => {
                        let path = config::expand(Path::new(&dest.location))?;
                        if path.is_dir() {
                            Ok(json!({"path":path}))
                        } else {
                            Err(anyhow::anyhow!("destination directory missing"))
                        }
                    }
                    DestinationKind::Sftp => {
                        let (host, path) = dest
                            .location
                            .split_once(':')
                            .context("invalid SFTP location")?;
                        let output = hostkit::ssh::Session::new(host)
                            .batch()
                            .script(&format!(
                                "test -d {} && test -w {}",
                                hostkit::shell::quote(path),
                                hostkit::shell::quote(path)
                            ))
                            .output_bounded(
                                hostkit::process::CaptureLimits {
                                    stdout: 4096,
                                    stderr: 4096,
                                },
                                Duration::from_secs(15),
                            )?;
                        if output.status.success() {
                            Ok(json!({"ssh":host,"writable":true}))
                        } else {
                            Err(anyhow::anyhow!("SSH storage path unavailable"))
                        }
                    }
                    DestinationKind::Drive => {
                        let remote = dest
                            .location
                            .split_once(':')
                            .context("invalid Drive remote")?
                            .0;
                        let mut command = Command::new(&config.tools.rclone);
                        if let Some(path) = &config.rclone_config_file {
                            command.env("RCLONE_CONFIG", path);
                        }
                        execute(
                            command.args(["about", "--json", &format!("{remote}:")]),
                            Duration::from_secs(30),
                        )
                        .and_then(|out| Ok(serde_json::from_slice::<Value>(&out.stdout)?))
                    }
                    DestinationKind::Rest => Ok(
                        json!({"append_only":dest.append_only,"verification":"repository commands verify access"}),
                    ),
                }
            })();
            match result {
                Ok(value) => checks.push(json!({"destination":name,"result":value})),
                Err(e) => errors.push(format!("{name}: {e:#}")),
            }
        }
    }
    Ok(json!({"checks":checks,"errors":errors}))
}

fn source_access(path: &Path) -> Result<()> {
    let result = (|| -> std::io::Result<()> {
        if std::fs::metadata(path)?.is_dir() {
            std::fs::read_dir(path)?.next().transpose()?;
        } else {
            std::fs::File::open(path)?;
        }
        Ok(())
    })();
    result.with_context(|| {
        if cfg!(target_os = "macos") {
            format!("source is not readable: {}; allow this terminal or app access in System Settings > Privacy & Security > Files and Folders", path.display())
        } else {
            format!("source is not readable: {}; check file and directory read/search permissions", path.display())
        }
    })
}

pub fn export(config: &Config, destination: &Path) -> Result<Value> {
    ensure!(!destination.exists(), "recovery destination already exists");
    private_dir(destination)?;
    let mut portable = config.clone();
    portable.runtime_digest = None;
    portable.state_dir = "~/.local/state/dcloud".into();
    let mut files = Vec::new();
    if let Some(source) = &config.secrets_file {
        copy_recovery_file(source, destination, "recovery.sops.json", &mut files)?;
        portable.secrets_file = Some("recovery.sops.json".into());
        portable.password_file = "~/.config/dcloud/repository.key".into();
        portable.identity_file = "~/.config/dcloud/identity.txt".into();
        let rules = source
            .ancestors()
            .skip(1)
            .map(|path| path.join(".sops.yaml"))
            .find(|path| path.is_file())
            .context("recovery export needs the repository .sops.yaml")?;
        copy_recovery_file(&rules, destination, ".sops.yaml", &mut files)?;
    } else {
        for (source, name) in [
            (&config.password_file, "repository.key"),
            (&config.identity_file, "identity.txt"),
        ] {
            check_secret(source)?;
            copy_recovery_file(source, destination, name, &mut files)?;
        }
        portable.password_file = "repository.key".into();
        portable.identity_file = "identity.txt".into();
    }
    if let Some(source) = &config.rclone_secrets_file {
        if source.try_exists()? {
            copy_recovery_file(source, destination, "rclone.sops.json", &mut files)?;
        }
        portable.rclone_secrets_file = Some("rclone.sops.json".into());
        portable.rclone_config_file = None;
    } else if let Some(source) = &config.rclone_config_file {
        check_secret(source)?;
        copy_recovery_file(source, destination, "rclone.conf", &mut files)?;
        portable.rclone_config_file = Some("rclone.conf".into());
    }
    write_private_new(
        &destination.join("config.toml"),
        toml::to_string_pretty(&portable)?.as_bytes(),
    )?;
    files.push("config.toml".into());
    Ok(
        json!({"directory":destination,"files":files,"encrypted":config.secrets_file.is_some(),"next":if config.secrets_file.is_some(){"keep an existing dotfile age identity separately to decrypt this recovery bundle"}else{"store this private recovery bundle separately from source computers"}}),
    )
}

fn copy_recovery_file(
    source: &Path,
    destination: &Path,
    name: &str,
    files: &mut Vec<String>,
) -> Result<()> {
    ensure!(
        fs::symlink_metadata(source)?.is_file(),
        "recovery input must be a regular file"
    );
    ensure!(
        fs::metadata(source)?.len() <= 16 * 1024 * 1024,
        "recovery file exceeds size limit"
    );
    write_private_new(&destination.join(name), &fs::read(source)?)?;
    files.push(name.into());
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/setup_tests.rs"]
mod tests;
