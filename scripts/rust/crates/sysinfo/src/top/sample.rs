//! Two process readings around the window: CPU time spent between them is the load.

use super::{Process, Sample, gpu, ps};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use sysinfo_backend::{
    CpuRefreshKind, MemoryRefreshKind, Pid, ProcessRefreshKind, ProcessesToUpdate, System,
    ThreadKind, UpdateKind,
};

pub fn sample(window: Duration) -> Sample {
    let gpu = gpu::Sampler::start(Instant::now() + window);
    let mut system = System::new();
    let first = refresh(&mut system);
    let deadline = first + window;
    let before: HashMap<Pid, u64> = system
        .processes()
        .iter()
        .map(|(pid, process)| (*pid, process.accumulated_cpu_time()))
        .collect();
    // Unreadable processes start at the epoch; `ps` reads them while the window is idle.
    let hidden: Vec<u32> = system
        .processes()
        .values()
        .filter(|process| process.start_time() == 0)
        .map(|process| process.pid().as_u32())
        .collect();
    let hidden_before = ps::read(&hidden);
    let hidden_after = hidden_before.as_ref().and_then(|reading| {
        sleep_until(deadline.checked_sub(reading.took).unwrap_or(deadline));
        ps::read(&hidden)
    });
    sleep_until(deadline);
    let second = refresh(&mut system);
    let window = second.saturating_duration_since(first);
    let hidden = match (hidden_before, hidden_after) {
        (Some(before), Some(after)) => ps::deltas(&before, &after, window),
        _ => HashMap::new(),
    };
    let gpu = gpu.finish();
    system.refresh_cpu_list(CpuRefreshKind::nothing());
    system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
    let own = Pid::from_u32(std::process::id());
    let processes = system
        .processes()
        .values()
        .filter(|process| {
            process.pid() != own
                && process.parent() != Some(own)
                && process.thread_kind() != Some(ThreadKind::Userland)
        })
        .map(|process| {
            let pid = process.pid().as_u32();
            let spent = process
                .accumulated_cpu_time()
                .saturating_sub(before.get(&process.pid()).copied().unwrap_or(0));
            let gpu = gpu.as_ref().and_then(|shares| shares.get(&pid).copied());
            if process.start_time() == 0
                && let Some(entry) = hidden.get(&pid)
            {
                return Process {
                    pid,
                    parent: Some(entry.parent),
                    uid: Some(entry.uid),
                    name: file_name(&entry.exe),
                    exe: entry.exe.starts_with('/').then(|| entry.exe.clone()),
                    command: Vec::new(),
                    kernel: false,
                    cpu_ms: entry.cpu_ms,
                    memory: entry.memory,
                    gpu,
                    age: entry.age,
                };
            }
            Process {
                pid,
                parent: process.parent().map(Pid::as_u32),
                uid: process.user_id().map(|uid| **uid),
                name: process.name().to_string_lossy().into_owned(),
                exe: process.exe().map(|exe| exe.to_string_lossy().into_owned()),
                command: process
                    .cmd()
                    .iter()
                    .map(|argument| argument.to_string_lossy().into_owned())
                    .collect(),
                kernel: process.thread_kind() == Some(ThreadKind::Kernel),
                cpu_ms: spent as f64,
                memory: memory(pid).unwrap_or_else(|| process.memory()),
                gpu,
                age: process.run_time(),
            }
        })
        .collect();
    Sample {
        host: System::host_name().unwrap_or_default(),
        cores: system.cpus().len(),
        memory: system.total_memory(),
        gpu: gpu.is_some(),
        window,
        processes,
    }
}

fn sleep_until(instant: Instant) {
    std::thread::sleep(instant.saturating_duration_since(Instant::now()));
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).into()
}

/// Refresh every process; the pass's midpoint stands for when it was read.
fn refresh(system: &mut System) -> Instant {
    let kind = ProcessRefreshKind::nothing()
        .without_tasks()
        .with_cpu()
        .with_memory()
        .with_user(UpdateKind::OnlyIfNotSet)
        .with_exe(UpdateKind::OnlyIfNotSet)
        .with_cmd(UpdateKind::OnlyIfNotSet);
    let began = Instant::now();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    began + began.elapsed() / 2
}

#[cfg(target_os = "macos")]
fn memory(pid: u32) -> Option<u64> {
    crate::collect::macos::footprint(pid)
}

#[cfg(not(target_os = "macos"))]
fn memory(_pid: u32) -> Option<u64> {
    None
}
