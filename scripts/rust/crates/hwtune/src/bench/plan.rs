use super::{families, json_output, runner, suites, text};
use clap::ArgMatches;
use serde::Serialize;
use std::path::{Path, PathBuf};
#[derive(Debug, Serialize)]
pub struct PlannedJob {
    pub name: String,
    pub family: String,
    pub available: bool,
    pub tools: Vec<String>,
    pub reason: String,
    pub expected_bytes_written: u64,
}
#[derive(Debug, Serialize)]
pub struct Plan {
    pub tier: String,
    pub workdir: PathBuf,
    pub write_budget: u64,
    pub expected_bytes_written: u64,
    pub jobs: Vec<PlannedJob>,
}
pub fn build(tier: &str, families: &[String], workdir: &Path) -> Plan {
    let accepts =
        |family: &str| families.is_empty() || families.iter().any(|selected| selected == family);
    let mut jobs = Vec::new();
    let mut add =
        |name: &str, family: &str, tools: &[&str], reason: &str, writes: u64, available: bool| {
            if accepts(family) {
                jobs.push(PlannedJob {
                    name: name.into(),
                    family: family.into(),
                    available: available && reason.is_empty(),
                    tools: tools.iter().map(|s| s.to_string()).collect(),
                    reason: if !reason.is_empty() {
                        reason.into()
                    } else if !available {
                        format!("missing dependency: {}", tools.join(" or "))
                    } else {
                        String::new()
                    },
                    expected_bytes_written: if available && reason.is_empty() {
                        writes
                    } else {
                        0
                    },
                });
            }
        };
    let seven = suites::tool_path(&["7z", "7zz"]).is_some();
    for name in ["cpu.single", "cpu.multi"] {
        add(name, "cpu", &["7z", "7zz"], "", 0, seven);
    }
    add(
        "cpu.crypto",
        "cpu",
        &["openssl"],
        "",
        0,
        suites::tool_path(&[
            "/opt/homebrew/opt/openssl@3/bin/openssl",
            "/usr/local/opt/openssl@3/bin/openssl",
            "openssl",
        ])
        .is_some(),
    );
    let native = suites::native::native_path();
    let native_reason = native.as_ref().err().cloned().unwrap_or_default();
    for (name, family) in [
        ("cpu.native", "cpu"),
        ("mem.native", "mem"),
        ("mem.latency", "mem"),
        ("sched.wake", "sched"),
    ] {
        add(
            name,
            family,
            &["bench-workloads"],
            &native_reason,
            0,
            native.as_ref().is_ok_and(|path| path.is_some()),
        );
    }
    for (name, family) in [
        ("mem.bandwidth", "mem"),
        ("mem.random", "mem"),
        ("cache.bandwidth", "cache"),
    ] {
        add(
            name,
            family,
            &["sysbench"],
            "",
            0,
            suites::tool_path(&["sysbench"]).is_some(),
        );
    }
    let disk_writes = suites::disk::parameters(tier).map_or(0, |(size, writes)| {
        suites::disk::predicted_writes(size, &writes)
    });
    add(
        "disk",
        "disk",
        &["fio"],
        if tier == "quick" {
            "excluded by quick tier"
        } else {
            ""
        },
        disk_writes,
        suites::tool_path(&["fio"]).is_some(),
    );
    let environment = std::env::vars().collect();
    let display = suites::gpu::has_display(&environment);
    let graphics_tools = suites::gpu::graphics_tools(&environment);
    add(
        "gpu.graphics",
        "gpu",
        graphics_tools,
        if !display {
            "no display in current environment; run also probes the user session"
        } else {
            ""
        },
        0,
        suites::tool_path(graphics_tools).is_some(),
    );
    add(
        "gpu.vulkan",
        "gpu",
        &["vkmark"],
        if tier == "quick" {
            "excluded by quick tier"
        } else if !display {
            "no display in current environment; run also probes the user session"
        } else {
            ""
        },
        0,
        suites::tool_path(&["vkmark"]).is_some(),
    );
    add(
        "gpu.compute",
        "gpu",
        &["swiftc"],
        if !cfg!(target_os = "macos") {
            "requires macOS Metal"
        } else {
            ""
        },
        0,
        suites::gpu::cached_metal(&suites::gpu::cache_dir()).is_some()
            || suites::tool_path(&["swiftc"]).is_some(),
    );
    let hyperfine = suites::tool_path(&["hyperfine"]).is_some();
    for (name, tool) in [
        ("workload.nvim_startup", "nvim"),
        ("workload.git_status", "git"),
        ("workload.git_log", "git"),
        ("workload.tar_repo", "tar"),
    ] {
        add(
            name,
            "workload",
            &["hyperfine", tool],
            "",
            0,
            hyperfine && suites::tool_path(&[tool]).is_some(),
        );
    }
    add(
        "thermal",
        "thermal",
        &["stress-ng"],
        if tier == "quick" {
            "excluded by quick tier"
        } else {
            ""
        },
        0,
        suites::tool_path(&["stress-ng"]).is_some(),
    );
    add(
        "compile",
        "compile",
        &["cargo", "rustc"],
        "",
        0,
        suites::tool_path(&["cargo"]).is_some(),
    );
    let sys = crate::env::Sysfs::from_env();
    let idle = suites::idle::Sources {
        rapl: crate::power::Rapl::discover(&sys).ok(),
        chip: crate::hwmon::Hwmon::find(&sys, crate::hwmon::CHIP).ok(),
        cpu: crate::hwmon::Hwmon::find(&sys, crate::hwmon::CPU_SENSOR).ok(),
        gpu: suites::tool_path(&["nvidia-smi"]).is_some(),
    };
    add("idle", "idle", &["hwtune"], "", 0, !idle.keys().is_empty());
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let model = crate::paths::Paths::discover(None)
        .ok()
        .and_then(|paths| {
            suites::ai::load_settings(&suites::ai::settings_path(&paths), home.as_deref())
                .ok()
                .flatten()
        })
        .is_some_and(|settings| settings.model.is_file());
    add(
        "ai",
        "ai",
        &["llama-cli"],
        if model {
            ""
        } else {
            "no ai model in config/hwtune/<host>.bench.dotfile"
        },
        0,
        suites::tool_path(&["llama-cli"]).is_some(),
    );
    Plan {
        tier: tier.into(),
        workdir: workdir.into(),
        write_budget: runner::write_budget(tier),
        expected_bytes_written: jobs.iter().map(|job| job.expected_bytes_written).sum(),
        jobs,
    }
}
pub fn run(args: &ArgMatches) -> Result<(), String> {
    let families = families(args)?;
    let workdir = if text(args, "workdir").is_empty() {
        runner::default_workdir()
    } else {
        PathBuf::from(text(args, "workdir"))
    };
    let plan = build(text(args, "tier"), &families, &workdir);
    if args.get_flag("json") {
        return json_output(&plan);
    }
    println!("{} benchmark plan", plan.tier);
    println!("workdir: {}", plan.workdir.display());
    println!(
        "expected writes: {:.1} GiB / {:.1} GiB budget",
        plan.expected_bytes_written as f64 / 1073741824.0,
        plan.write_budget as f64 / 1073741824.0
    );
    for job in plan.jobs {
        println!(
            "  {:24} {}{}",
            job.name,
            if job.available { "available" } else { "skip: " },
            job.reason
        );
    }
    Ok(())
}
