use super::{Job, Measurement, capture, job, output, require, tool_path, version};
use crate::bench::provenance::sha256;
use crate::bench::runner::Setting;
use crate::env::Sysfs;
use crate::power::Rapl;
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

pub const MANIFEST: &str = include_str!("compile/Cargo.toml");
pub const LOCKFILE: &str = include_str!("compile/Cargo.lock");
pub const SOURCE: &str = include_str!("compile/main.rs");
pub const METHOD: &str = "compile.pinned/1.0.0";

pub fn lock_packages(lock: &str) -> usize {
    lock.lines().filter(|line| *line == "[[package]]").count()
}

pub fn lock_sha(lock: &str) -> String {
    sha256(lock.as_bytes())
}

pub fn workspace(workdir: &Path) -> PathBuf {
    workdir.join("compile")
}

pub fn target_dir(root: &Path) -> PathBuf {
    root.join("target")
}

pub fn write_if_changed(path: &Path, content: &str) -> Result<bool, String> {
    if fs::read_to_string(path).is_ok_and(|current| current == content) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(path, content).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

pub fn write_assets(root: &Path) -> Result<(), String> {
    for (name, content) in [
        ("Cargo.toml", MANIFEST),
        ("Cargo.lock", LOCKFILE),
        ("src/main.rs", SOURCE),
    ] {
        write_if_changed(&root.join(name), content)?;
    }
    Ok(())
}

pub fn clean_target(root: &Path) -> Result<(), String> {
    let target = target_dir(root);
    match fs::remove_dir_all(&target) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", target.display())),
    }
}

pub fn build_command(cargo: &Path, root: &Path, release: bool) -> Command {
    let mut command = Command::new(cargo);
    command.args(["build", "-q", "--locked", "--offline"]);
    if release {
        command.arg("--release");
    }
    command
        .current_dir(root)
        .env("CARGO_TARGET_DIR", target_dir(root))
        .env("CARGO_BUILD_BUILD_DIR", target_dir(root))
        .env("CARGO_INCREMENTAL", "0")
        .env("RUSTC_WRAPPER", "")
        .env("RUSTFLAGS", "");
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        command.env("CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER", "cc");
    }
    command
}

fn build_ms(cargo: &Path, root: &Path, release: bool) -> Result<f64, String> {
    clean_target(root)?;
    let started = Instant::now();
    require(&mut build_command(cargo, root, release), 1800)?;
    Ok(started.elapsed().as_secs_f64() * 1000.0)
}

fn fetched(cargo: &Path, root: &Path) -> bool {
    for (args, seconds) in [
        (["fetch", "--locked", "--offline"].as_slice(), 120),
        (["fetch", "--locked"].as_slice(), 300),
    ] {
        if capture(Command::new(cargo).args(args).current_dir(root), seconds)
            .is_ok_and(|result| result.status.success())
        {
            return true;
        }
    }
    false
}

pub fn jobs(setting: &Setting) -> Result<Vec<Job>, String> {
    if !setting.accepts("compile") {
        return Ok(Vec::new());
    }
    let Some(cargo) = tool_path(&["cargo"]) else {
        return Ok(Vec::new());
    };
    let root = workspace(&setting.workdir);
    write_assets(&root)?;
    if !fetched(&cargo, &root) {
        return Ok(Vec::new());
    }
    let rustc = tool_path(&["rustc"])
        .map(|path| version(&path, &["-V"], r"rustc\s+(\d[\d.]*)"))
        .unwrap_or_default();
    let rapl = Rapl::discover(&Sysfs::from_env()).ok();
    let mut outputs = vec![
        output("compile.dev", "ms", "LIB", "world"),
        output("compile.release", "ms", "LIB", "world"),
    ];
    if rapl.is_some() {
        outputs.push(output("compile.package_j", "J", "LIB", "host"));
    }
    let detail = json!({
        "packages": lock_packages(LOCKFILE),
        "lockfile_sha256": lock_sha(LOCKFILE),
        "profiles": ["dev", "release"],
        "jobs": std::thread::available_parallelism().map_or(1, usize::from),
    });
    Ok(vec![job(
        "compile",
        "rustc",
        &rustc,
        METHOD,
        outputs,
        detail,
        move || {
            let mut values = Vec::new();
            let mut joules = 0.0;
            for (key, release) in [("compile.dev", false), ("compile.release", true)] {
                let elapsed = match &rapl {
                    Some(rapl) => {
                        let (elapsed, energy) =
                            rapl.measure(|| build_ms(&cargo, &root, release))?;
                        joules += energy.joules;
                        elapsed?
                    }
                    None => build_ms(&cargo, &root, release)?,
                };
                values.push((key.to_string(), vec![elapsed]));
            }
            if rapl.is_some() {
                values.push(("compile.package_j".to_string(), vec![joules]));
            }
            Ok(Measurement::values(values))
        },
    )])
}

#[cfg(test)]
#[path = "../../../tests/unit/bench/compile_tests.rs"]
mod tests;
