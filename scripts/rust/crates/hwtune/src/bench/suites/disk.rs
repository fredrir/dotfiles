use super::{Job, Measurement, WRITTEN, capture, job, output, parse_size, tool_path, version};
use crate::bench::runner::Setting;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path, process::Command};
pub const STAGES: [(&str, &str, &str, &str); 4] = [
    ("seq-read", "read", "1m", "disk.seq_read"),
    ("seq-write", "write", "1m", "disk.seq_write"),
    ("rand-read", "randread", "4k", "disk.rand_read"),
    ("rand-write", "randwrite", "4k", "disk.rand_write"),
];
pub fn engine() -> &'static str {
    if cfg!(target_os = "macos") {
        "posixaio"
    } else {
        "libaio"
    }
}
pub fn job_file(
    size: &str,
    target: &Path,
    engine: &str,
    writes: &BTreeMap<String, String>,
) -> String {
    let mut lines = vec![
        "[global]".to_string(),
        format!("ioengine={engine}"),
        "direct=1".into(),
        format!("size={size}"),
        "iodepth=64".into(),
        "ramp_time=5".into(),
        "group_reporting=1".into(),
        format!("directory={}", target.display()),
    ];
    if !cfg!(target_os = "macos") {
        lines.push("disk_util=0".into());
    }
    lines.push(String::new());
    for (name, mode, block, _) in STAGES {
        lines.extend([
            format!("[{name}]"),
            format!("rw={mode}"),
            format!("bs={block}"),
        ]);
        if let Some(limit) = writes.get(name) {
            lines.push(format!("io_size={limit}"));
        } else {
            lines.extend(["time_based=1".into(), "runtime=20".into()]);
        }
        lines.extend(["stonewall".into(), String::new()]);
    }
    lines.join("\n")
}
pub fn parse(payload: &Value) -> Result<Measurement, String> {
    let mut values = BTreeMap::new();
    let mut written = 0.0;
    for entry in payload["jobs"].as_array().into_iter().flatten() {
        if entry["error"].as_i64().is_some_and(|code| code != 0) {
            return Err(format!(
                "fio job {} failed",
                entry["jobname"].as_str().unwrap_or("unknown")
            ));
        }
        written += entry["write"]["io_bytes"].as_f64().unwrap_or(0.0);
        for (stage, _, _, key) in STAGES {
            if entry["jobname"] != stage {
                continue;
            }
            let side = &entry[if stage.contains("write") {
                "write"
            } else {
                "read"
            }];
            let value = if stage.contains("rand") {
                side["iops"].as_f64().unwrap_or(0.0)
            } else {
                side["bw_bytes"].as_f64().unwrap_or(0.0) / 1_000_000.0
            };
            values.insert(key.into(), vec![value]);
            if let Some(latency) = side["clat_ns"]["percentile"]["99.000000"]
                .as_f64()
                .filter(|n| *n != 0.0)
            {
                values.insert(format!("{key}_p99"), vec![latency / 1000.0]);
            }
        }
    }
    if values.is_empty() {
        return Err("fio produced no usable results".into());
    }
    values.insert(WRITTEN.into(), vec![written]);
    Ok(Measurement {
        values,
        detail: json!({}),
    })
}
pub fn predicted_writes(size: &str, writes: &BTreeMap<String, String>) -> u64 {
    parse_size(size) * 4 + writes.values().map(|value| parse_size(value)).sum::<u64>()
}
pub fn parameters(tier: &str) -> Option<(&'static str, BTreeMap<String, String>)> {
    let (size, seq, random) = match tier {
        "standard" => ("1g", "6g", "2g"),
        "heavy" => ("8g", "20g", "6g"),
        _ => return None,
    };
    Some((
        size,
        BTreeMap::from([
            ("seq-write".into(), seq.into()),
            ("rand-write".into(), random.into()),
        ]),
    ))
}
pub fn retry_before_writes(directory: &Path, payload: &[u8]) -> bool {
    // fio lays out files before it can write. Only an engine rejected before
    // layout is safe to retry without consuming a second disk budget.
    let empty = fs::read_dir(directory).is_ok_and(|entries| {
        entries
            .into_iter()
            .all(|entry| entry.is_ok_and(|entry| entry.file_name() == "bench.fio"))
    });
    let wrote = serde_json::from_slice::<Value>(payload)
        .ok()
        .is_some_and(|value| {
            value["jobs"].as_array().into_iter().flatten().any(|job| {
                job["write"]["io_bytes"]
                    .as_f64()
                    .is_some_and(|bytes| bytes > 0.0)
            })
        });
    empty && !wrote
}
pub fn jobs(setting: &Setting) -> Result<Vec<Job>, String> {
    if !setting.accepts("disk") || setting.tier == "quick" {
        return Ok(Vec::new());
    }
    let Some(path) = tool_path(&["fio"]) else {
        return Ok(Vec::new());
    };
    let Some((size, writes)) = parameters(&setting.tier) else {
        return Ok(Vec::new());
    };
    let predicted = predicted_writes(size, &writes);
    let target = setting.workdir.clone();
    let version = version(&path, &["--version"], r"fio-(\d[\d.]*)");
    let outputs = STAGES
        .iter()
        .flat_map(|(_, _, _, key)| {
            vec![
                output(
                    key,
                    if key.contains("rand") { "IOPS" } else { "MB/s" },
                    "HIB",
                    "host",
                ),
                output(&format!("{key}_p99"), "µs", "LIB", "host"),
            ]
        })
        .collect();
    let mut result = job(
        "disk",
        "fio",
        &version,
        "disk/2.0.0",
        outputs,
        json!({"size":size,"runtime":20,"ramp_time":5,"iodepth":64,"direct":1,"engine":engine(),"write_size":writes}),
        move || {
            let scratch = tempfile::Builder::new()
                .prefix("sysinfo-fio-")
                .tempdir_in(&target)
                .map_err(|e| e.to_string())?;
            let spec = scratch.path().join("bench.fio");
            let mut refused = Vec::new();
            for ioengine in [engine(), "psync"] {
                fs::write(&spec, job_file(size, scratch.path(), ioengine, &writes))
                    .map_err(|e| e.to_string())?;
                let reply = capture(
                    Command::new(&path).arg("--output-format=json").arg(&spec),
                    900,
                )?;
                if !reply.status.success() {
                    let reason = String::from_utf8_lossy(&reply.stderr)
                        .lines()
                        .rev()
                        .find(|line| !line.trim().is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("exited {}", reply.status));
                    refused.push(format!("{ioengine}: {reason}"));
                    if !retry_before_writes(scratch.path(), &reply.stdout) {
                        return Err(format!(
                            "{}; not retrying after fio started disk writes",
                            refused.join("; ")
                        ));
                    }
                    continue;
                }
                let payload: Value = serde_json::from_slice(&reply.stdout)
                    .map_err(|_| "fio produced unreadable output")?;
                let mut result = parse(&payload)?;
                let layout = fs::read_dir(scratch.path())
                    .map_err(|e| e.to_string())?
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        STAGES.iter().any(|(stage, _, _, _)| {
                            entry
                                .file_name()
                                .to_string_lossy()
                                .starts_with(&format!("{stage}."))
                        })
                    })
                    .filter_map(|entry| entry.metadata().ok())
                    .map(|m| m.len())
                    .sum::<u64>();
                result
                    .values
                    .entry(WRITTEN.into())
                    .or_insert_with(|| vec![0.0])[0] += if layout == 0 {
                    parse_size(size) * 4
                } else {
                    layout
                } as f64;
                result.detail = json!({"engine":ioengine});
                return Ok(result);
            }
            Err(format!(
                "fio could not run any I/O engine ({})",
                refused.join("; ")
            ))
        },
    );
    result.writes = predicted;
    result.repeat = false;
    Ok(vec![result])
}
