use super::{Job, Measurement, capture, job, output, tool_path, version};
use crate::bench::runner::Setting;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
pub fn displayed(command: &[String], directory: &Path) -> String {
    command
        .iter()
        .map(|part| {
            if part == &directory.to_string_lossy() {
                "."
            } else {
                part.as_str()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
pub fn candidates(root: &Path) -> Vec<(String, Vec<String>)> {
    let mut found = Vec::new();
    let mut push = |key: &str, tool: &str, args: Vec<String>| {
        if tool_path(&[tool]).is_some() {
            let mut command = vec![tool.into()];
            command.extend(args);
            found.push((key.into(), command));
        }
    };
    push(
        "workload.nvim_startup",
        "nvim",
        vec!["--headless".into(), "+qa".into()],
    );
    push(
        "workload.git_status",
        "git",
        vec![
            "-C".into(),
            root.display().to_string(),
            "status".into(),
            "--porcelain".into(),
        ],
    );
    push(
        "workload.git_log",
        "git",
        vec![
            "-C".into(),
            root.display().to_string(),
            "log".into(),
            "--oneline".into(),
            "-n".into(),
            "200".into(),
        ],
    );
    push(
        "workload.tar_repo",
        "tar",
        vec![
            "-cf".into(),
            "/dev/null".into(),
            "scripts/python/src".into(),
        ],
    );
    found
}
pub fn timings(path: &Path, command: &[String], directory: &Path) -> Result<Vec<f64>, String> {
    let scratch = tempfile::Builder::new()
        .prefix("bench-workload-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let export = scratch.path().join("result.json");
    let shell = command
        .iter()
        .map(|arg| hostkit::shell::quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let result = capture(
        Command::new(path)
            .args(["-N", "--warmup", "2", "--runs", "5", "--export-json"])
            .arg(&export)
            .args(["--command-name", "workload"])
            .arg(shell)
            .current_dir(directory),
        180,
    )?;
    if !result.status.success() {
        return Err(format!("hyperfine exited {}", result.status));
    }
    let payload: Value = serde_json::from_slice(
        &fs::read(export).map_err(|_| "hyperfine produced unreadable output")?,
    )
    .map_err(|_| "hyperfine produced unreadable output")?;
    let row = payload["results"]
        .as_array()
        .and_then(|rows| rows.first())
        .ok_or("hyperfine reported no results")?;
    let times = if let Some(times) = row["times"].as_array().filter(|times| !times.is_empty()) {
        times.iter().filter_map(Value::as_f64).collect::<Vec<_>>()
    } else {
        row["median"].as_f64().into_iter().collect()
    };
    if times.is_empty() || times.iter().any(|n| !n.is_finite() || *n < 0.0) {
        return Err("hyperfine reported invalid timings".into());
    }
    Ok(times.into_iter().map(|n| n * 1000.0).collect())
}
pub fn jobs(setting: &Setting) -> Result<Vec<Job>, String> {
    if !setting.accepts("workload") {
        return Ok(Vec::new());
    }
    let Some(path) = tool_path(&["hyperfine"]) else {
        return Ok(Vec::new());
    };
    let ver = version(&path, &["--version"], r"(\d[\d.]*)");
    let root = crate::inventory::repo_root();
    let mut jobs = Vec::new();
    for (key, args) in candidates(&root) {
        if !capture(
            Command::new(&args[0]).args(&args[1..]).current_dir(&root),
            20,
        )
        .is_ok_and(|r| r.status.success())
        {
            continue;
        }
        let binary: PathBuf = path.clone();
        let directory = root.clone();
        let id = key.clone();
        let mut result = job(
            &key,
            "hyperfine",
            &ver,
            &format!("{key}/1.0.0"),
            vec![output(&key, "ms", "LIB", "host")],
            json!({"command":displayed(&args,&root),"runs":"5","warmup":"2"}),
            move || {
                Ok(Measurement::values([(
                    id.clone(),
                    timings(&binary, &args, &directory)?,
                )]))
            },
        );
        result.repeat = false;
        jobs.push(result);
    }
    Ok(jobs)
}
