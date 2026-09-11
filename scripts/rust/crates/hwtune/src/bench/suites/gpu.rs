use super::{Job, capture, executable, job, number, output, require, scalar, tool_path, version};
use crate::bench::runner::Setting;
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};
const METAL_SOURCE: &str = include_str!("metal/gpu_bench.swift");
pub fn has_display(env: &BTreeMap<String, String>) -> bool {
    ["WAYLAND_DISPLAY", "DISPLAY"]
        .iter()
        .any(|key| env.get(*key).is_some_and(|value| !value.is_empty()))
}
pub fn graphics_tools(env: &BTreeMap<String, String>) -> &'static [&'static str] {
    if env
        .get("WAYLAND_DISPLAY")
        .is_some_and(|value| !value.is_empty())
    {
        &["glmark2-wayland", "glmark2-es2-wayland"]
    } else if has_display(env) {
        &["glmark2", "glmark2-es2"]
    } else {
        &[]
    }
}
pub fn session_environment() -> BTreeMap<String, String> {
    let mut env = std::env::vars().collect::<BTreeMap<_, _>>();
    if env.get("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        || env.get("DISPLAY").is_some_and(|v| !v.is_empty())
    {
        return env;
    }
    let runtime = env.get("XDG_RUNTIME_DIR").cloned().unwrap_or_else(|| {
        require(Command::new("id").arg("-u"), 5)
            .map(|uid| format!("/run/user/{}", uid.trim()))
            .unwrap_or_default()
    });
    if let Ok(result) = capture(
        Command::new("systemctl")
            .args(["--user", "show-environment"])
            .env("XDG_RUNTIME_DIR", &runtime),
        5,
    ) && result.status.success()
    {
        for line in String::from_utf8_lossy(&result.stdout).lines() {
            if let Some((key, value)) = line.split_once('=')
                && ["WAYLAND_DISPLAY", "DISPLAY", "XDG_RUNTIME_DIR"].contains(&key)
                && !value.is_empty()
            {
                env.insert(key.into(), value.into());
            }
        }
        env.entry("XDG_RUNTIME_DIR".into()).or_insert(runtime);
    }
    env
}
pub fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
        })
        .join("hwtune/bench")
}
pub fn cached_metal(directory: &Path) -> Option<PathBuf> {
    let binary = directory.join("gpu_bench");
    (executable(&binary)
        && fs::read_to_string(directory.join("gpu_bench.swift"))
            .is_ok_and(|text| text == METAL_SOURCE))
    .then_some(binary)
}
fn metal_binary() -> Result<Option<PathBuf>, String> {
    if !cfg!(target_os = "macos") {
        return Ok(None);
    }
    let directory = cache_dir();
    if let Some(binary) = cached_metal(&directory) {
        return Ok(Some(binary));
    }
    metal_binary_at(&directory, tool_path(&["swiftc"]).as_deref())
}
pub(crate) fn metal_binary_at(
    directory: &Path,
    compiler: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    if let Some(binary) = cached_metal(directory) {
        return Ok(Some(binary));
    }
    let Some(compiler) = compiler else {
        return Ok(None);
    };
    fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let stored = directory.join("gpu_bench.swift");
    let binary = directory.join("gpu_bench");
    let scratch = tempfile::Builder::new()
        .prefix("metal-build-")
        .tempdir_in(directory)
        .map_err(|e| e.to_string())?;
    let input = scratch.path().join("gpu_bench.swift");
    let output = scratch.path().join("gpu_bench");
    fs::write(&input, METAL_SOURCE).map_err(|e| e.to_string())?;
    require(
        Command::new(compiler)
            .arg("-O")
            .arg(&input)
            .arg("-o")
            .arg(&output),
        600,
    )?;
    fs::rename(output, &binary).map_err(|e| e.to_string())?;
    crate::bench::store::atomic_write(&stored, METAL_SOURCE.as_bytes())?;
    Ok(Some(binary))
}
pub fn jobs(setting: &Setting) -> Result<Vec<Job>, String> {
    if !setting.accepts("gpu") {
        return Ok(Vec::new());
    }
    let mut jobs = Vec::new();
    let env = session_environment();
    let display = has_display(&env);
    let glmark = tool_path(graphics_tools(&env));
    if let Some(path) = glmark {
        let ver = version(&path, &["--version"], r"(\d[\d.]*)");
        let tool = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let environment = env.clone();
        let mut result = job(
            "gpu.graphics",
            &tool,
            &ver,
            "gpu.graphics/1.0.0",
            vec![output("gpu.graphics", "score", "HIB", "host")],
            json!({"scenes":["build:duration=2","texture:duration=2","shading:duration=2"],"mode":"off-screen"}),
            move || {
                let text = require(
                    Command::new(&path)
                        .args([
                            "--off-screen",
                            "-b",
                            "build:duration=2",
                            "-b",
                            "texture:duration=2",
                            "-b",
                            "shading:duration=2",
                        ])
                        .envs(&environment),
                    600,
                )?;
                Ok(scalar(
                    "gpu.graphics",
                    number(&text, r"Score:\s*(\d+)", "glmark2 reported no score")?,
                ))
            },
        );
        result.repeat = false;
        jobs.push(result);
    }
    if display
        && setting.tier != "quick"
        && let Some(path) = tool_path(&["vkmark"])
    {
        let ver = version(&path, &["--version"], r"(\d[\d.]*)");
        let environment = env.clone();
        let mut result = job(
            "gpu.vulkan",
            "vkmark",
            &ver,
            "gpu.vulkan/1.0.0",
            vec![output("gpu.vulkan", "score", "HIB", "host")],
            json!({}),
            move || {
                let text = require(Command::new(&path).envs(&environment), 600)?;
                Ok(scalar(
                    "gpu.vulkan",
                    number(&text, r"Score:\s*(\d+)", "vkmark reported no score")?,
                ))
            },
        );
        result.repeat = false;
        jobs.push(result);
    }
    let metal = match metal_binary() {
        Ok(binary) => binary,
        Err(error) if crate::bench::runner::cancelled() => return Err(error),
        Err(error) => {
            eprintln!("  gpu.compute skipped: {error}");
            None
        }
    };
    if let Some(path) = metal {
        let mut result = job(
            "gpu.compute",
            "metal",
            "1.0.0",
            "gpu.compute/1.0.0",
            vec![output("gpu.compute", "GFLOPS", "HIB", "host")],
            json!({"kernel":"fma","api":"Metal"}),
            move || {
                let text = require(&mut Command::new(&path), 300)?;
                let regex = regex::Regex::new(r"[\d.]+").expect("number regex");
                let value = regex
                    .find_iter(&text)
                    .last()
                    .and_then(|m| m.as_str().parse::<f64>().ok())
                    .ok_or("gpu_bench reported no throughput")?;
                Ok(scalar("gpu.compute", value))
            },
        );
        result.repeat = false;
        jobs.push(result);
    }
    Ok(jobs)
}
