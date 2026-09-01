use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use hostkit::{Host, Route};

fn main() {
    let binary = release_binary();
    let this = Host::this().expect("hwire info benchmarks require macOS or Linux");
    let ssh_connection = format!(
        "{} 54321 {} 22",
        this.peer().address(Route::Cable).unwrap(),
        this.address(Route::Cable).unwrap()
    );

    let remote = median(50, || {
        Command::new(&binary)
            .args(["-i", "--color", "never"])
            .env("SSH_CONNECTION", &ssh_connection)
            .env_remove("HWIRE_SESSION")
            .stdin(Stdio::null())
            .output()
            .expect("run established-session info")
    });
    gate("established remote", remote, Duration::from_millis(3));

    let local = median(15, || {
        Command::new(&binary)
            .args(["-i", "--color", "never"])
            .env_remove("SSH_CONNECTION")
            .env_remove("HWIRE_SESSION")
            .stdin(Stdio::null())
            .output()
            .expect("run local route info")
    });
    gate("healthy local", local, Duration::from_millis(75));
}

fn median(samples: usize, mut command: impl FnMut() -> std::process::Output) -> Duration {
    let warmup = command();
    assert!(
        warmup.status.success(),
        "benchmark warmup failed: {warmup:?}"
    );
    let mut elapsed = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let output = command();
        let duration = started.elapsed();
        assert!(
            output.status.success(),
            "benchmark sample failed: {output:?}"
        );
        elapsed.push(duration);
    }
    elapsed.sort_unstable();
    elapsed[elapsed.len() / 2]
}

fn gate(label: &str, measured: Duration, budget: Duration) {
    println!(
        "{label}: median {:.2} ms (budget {:.0} ms)",
        measured.as_secs_f64() * 1_000.0,
        budget.as_secs_f64() * 1_000.0
    );
    assert!(
        measured <= budget,
        "{label} median {:.2} ms exceeded {:.0} ms budget",
        measured.as_secs_f64() * 1_000.0,
        budget.as_secs_f64() * 1_000.0
    );
}

fn release_binary() -> PathBuf {
    let executable = std::env::current_exe().expect("locate benchmark executable");
    let release = executable
        .parent()
        .and_then(|deps| deps.parent())
        .expect("benchmark is under target/release/deps");
    let binary = release.join(format!("hwire{}", std::env::consts::EXE_SUFFIX));
    assert!(
        binary.is_file(),
        "{} is missing; run `cargo build --release -p hwire` first",
        binary.display()
    );
    binary
}
