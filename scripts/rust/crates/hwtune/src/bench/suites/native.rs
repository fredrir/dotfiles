use super::{Job, Measurement, job, output, require, version};
use crate::bench::runner::Setting;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
pub fn native_path() -> Result<Option<PathBuf>, String> {
    workstation::native::Resolver::discover(sysinfo::inventory::repo_root())
        .resolve("bench-workloads")
}
pub fn value_of(text: &str) -> Result<f64, String> {
    let payload: Value =
        serde_json::from_str(text).map_err(|_| "bench-workloads produced unreadable output")?;
    payload["value"]
        .as_f64()
        .filter(|n| n.is_finite() && *n > 0.0)
        .ok_or_else(|| "bench-workloads reported no value".into())
}
pub fn measure(binary: &Path, args: &[&str], seconds: u64) -> Result<f64, String> {
    value_of(&require(Command::new(binary).args(args), seconds)?)
}
pub fn jobs(setting: &Setting) -> Result<Vec<Job>, String> {
    if !setting.accepts("cpu") && !setting.accepts("mem") {
        return Ok(Vec::new());
    }
    let Some(path) = native_path()? else {
        return Ok(Vec::new());
    };
    let ver = version(&path, &["--version"], r"(\d[\d.]*)");
    let mut jobs = Vec::new();
    for (name, scale, proportion, detail, measurements) in [
        (
            "cpu.native",
            "Mops/s",
            "HIB",
            json!({"iterations":800000000}),
            vec![
                (
                    "cpu.native_single",
                    vec!["cpu", "--threads", "1", "--iterations", "800000000"],
                ),
                (
                    "cpu.native_multi",
                    vec!["cpu", "--threads", "0", "--iterations", "800000000"],
                ),
            ],
        ),
        (
            "mem.native",
            "GiB/s",
            "HIB",
            json!({"buffer_mib":256,"passes":128,"threads":1}),
            vec![
                (
                    "mem.native_read",
                    vec!["memory", "--op", "read", "--mib", "256", "--passes", "128"],
                ),
                (
                    "mem.native_write",
                    vec!["memory", "--op", "write", "--mib", "256", "--passes", "128"],
                ),
            ],
        ),
        (
            "mem.latency",
            "ns",
            "LIB",
            json!({"buffer_mib":256,"passes":4,"threads":1,"access":"dependent pointer chase"}),
            vec![(
                "mem.latency",
                vec!["memory", "--op", "latency", "--mib", "256", "--passes", "4"],
            )],
        ),
    ] {
        let binary = path.clone();
        let outputs = measurements
            .iter()
            .map(|(key, _)| output(key, scale, proportion, "world"))
            .collect();
        jobs.push(job(
            name,
            "bench-workloads",
            &ver,
            &format!("{name}/1.0.0"),
            outputs,
            detail,
            move || {
                let mut values = Vec::new();
                for (key, args) in &measurements {
                    values.push((key.to_string(), vec![measure(&binary, args, 300)?]));
                }
                Ok(Measurement::values(values))
            },
        ));
    }
    Ok(jobs)
}

#[cfg(test)]
#[path = "../../../tests/unit/bench/native_tests.rs"]
mod tests;
