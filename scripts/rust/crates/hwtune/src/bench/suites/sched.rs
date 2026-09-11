use super::{Job, Measurement, job, native, output, version};
use crate::bench::runner::Setting;
use serde_json::json;

pub const ITERATIONS: &str = "2000";
pub const SLEEP_US: &str = "1000";

pub fn wake_args(loaded: bool) -> Vec<&'static str> {
    let mut args = vec!["wake", "--iterations", ITERATIONS, "--sleep-us", SLEEP_US];
    args.extend(["--load", if loaded { "all" } else { "none" }]);
    args
}

pub fn jobs(setting: &Setting) -> Result<Vec<Job>, String> {
    if !setting.accepts("sched") {
        return Ok(Vec::new());
    }
    let Some(path) = native::native_path()? else {
        return Ok(Vec::new());
    };
    let ver = version(&path, &["--version"], r"(\d[\d.]*)");
    let outputs = ["sched.wake_idle", "sched.wake_loaded"]
        .iter()
        .map(|key| output(key, "us", "LIB", "host"))
        .collect();
    Ok(vec![job(
        "sched.wake",
        "bench-workloads",
        &ver,
        "sched.wake/1.0.0",
        outputs,
        json!({"iterations":ITERATIONS,"sleep_us":SLEEP_US,"statistic":"p99 oversleep"}),
        move || {
            let mut values = Vec::new();
            for (key, loaded) in [("sched.wake_idle", false), ("sched.wake_loaded", true)] {
                values.push((
                    key.to_string(),
                    vec![native::measure(&path, &wake_args(loaded), 60)?],
                ));
            }
            Ok(Measurement::values(values))
        },
    )])
}

#[cfg(test)]
#[path = "../../../tests/unit/bench/sched_tests.rs"]
mod tests;
