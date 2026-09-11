#![cfg(unix)]
#![forbid(unsafe_code)]

use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use hostkit::process::{self, CaptureLimits};
use hwtune::env::Sysfs;
use hwtune::tune::monitor::{self, Monitor};
use serde_json::{Value, json};

const DEADLINE: Duration = Duration::from_secs(15);

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn fixture(case: &str) -> (tempfile::TempDir, Value) {
    let root = tempfile::tempdir().unwrap();
    let sensor = root.path().join("sys/class/hwmon/hwmon0");
    write(&sensor.join("name"), "k10temp\n");
    write(&sensor.join("temp1_input"), "35000\n");
    write(&sensor.join("temp1_max"), "90000\n");
    write(&sensor.join("temp1_label"), "CPU package\n");
    let bin = root.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let journal = if case == "invisible" {
        "#!/bin/sh\nexit 0\n"
    } else {
        "#!/bin/sh\nprintf '%s\\n' '{\"__CURSOR\":\"cursor-1\",\"MESSAGE\":\"kernel initialized\"}'\n"
    };
    testkit::executable(&bin.join("journalctl"), journal);
    let output = process::output(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "monitor_helper", "--nocapture"])
            .env("HWTUNE_MONITOR_TEST_ROLE", "monitor")
            .env("HWTUNE_MONITOR_TEST_CASE", case)
            .env("HWTUNE_MONITOR_TEST_ROOT", root.path())
            .env("HWTUNE_SYSFS_ROOT", root.path().join("sys"))
            .env(
                "HWTUNE_MEASUREMENT_LOCK",
                root.path().join("measurement.lock"),
            )
            .env("HWTUNE_BENCHMARKS", root.path().join("history"))
            .env("PATH", bin),
        CaptureLimits::default(),
        Duration::from_secs(30),
    )
    .unwrap();
    assert!(
        output.status.success(),
        "{case}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let result =
        serde_json::from_slice(&fs::read(root.path().join("result.json")).unwrap()).unwrap();
    (root, result)
}

#[test]
fn missing_kernel_journal_visibility_rejects_before_workload() {
    let (root, result) = fixture("invisible");
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("visibility unavailable")
    );
    assert!(!root.path().join("workload-started").exists());
}

#[test]
fn readable_telemetry_finishes_with_complete_evidence() {
    let (root, result) = fixture("healthy");
    assert!(root.path().join("workload-started").exists());
    assert_eq!(result["workload_success"], true);
    assert_eq!(result["evidence"]["passed"], true);
    assert_eq!(result["evidence"]["peak_temp_c"], 35.0);
    assert!(result["evidence"]["samples"].as_u64().unwrap() >= 2);
}

#[test]
fn unsafe_telemetry_cancels_workload_and_its_descendant_group() {
    let (_, result) = fixture("unsafe");
    assert_eq!(result["interrupted"], true);
    assert_eq!(result["descendant_stopped"], true);
    assert_eq!(result["evidence"]["passed"], false);
    assert_eq!(result["evidence"]["peak_temp_c"], 100.0);
    assert!(
        result["evidence"]["reason"]
            .as_str()
            .unwrap()
            .contains("reached")
    );
}

fn helper_root(role: &str) -> Option<PathBuf> {
    (std::env::var("HWTUNE_MONITOR_TEST_ROLE").as_deref() == Ok(role))
        .then(|| PathBuf::from(std::env::var_os("HWTUNE_MONITOR_TEST_ROOT").unwrap()))
}

fn helper_command(role: &str, test: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", test, "--nocapture"])
        .env("HWTUNE_MONITOR_TEST_ROLE", role);
    command
}

fn connection(listener: &UnixListener) -> UnixStream {
    listener.set_nonblocking(true).unwrap();
    let until = Instant::now() + DEADLINE;
    loop {
        match listener.accept() {
            Ok((socket, _)) => return socket,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < until,
                    "workload descendant did not connect"
                );
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("workload rendezvous failed: {error}"),
        }
    }
}

#[test]
fn monitor_helper() {
    let Some(root) = helper_root("monitor") else {
        return;
    };
    let sys = Sysfs::from_env();
    let monitor = match Monitor::start(&sys, Some(80.0)) {
        Ok(monitor) => monitor,
        Err(error) => {
            write(
                &root.join("result.json"),
                &json!({"error": error}).to_string(),
            );
            return;
        }
    };
    write(&root.join("workload-started"), "started");
    let case = std::env::var("HWTUNE_MONITOR_TEST_CASE").unwrap();
    let result = if case == "unsafe" {
        let listener = UnixListener::bind(root.join("rendezvous.sock")).unwrap();
        let task = thread::spawn(|| {
            process::output_cancellable(
                &mut helper_command("workload", "workload_helper"),
                CaptureLimits::default(),
                DEADLINE,
                &monitor::cancelled,
            )
        });
        let mut socket = connection(&listener);
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut ready = [0; 5];
        socket.read_exact(&mut ready).unwrap();
        assert_eq!(&ready, b"ready");
        let sensor = root.join("sys/class/hwmon/hwmon0/temp1_input");
        let replacement = sensor.with_extension("next");
        write(&replacement, "100000\n");
        fs::rename(replacement, sensor).unwrap();
        let captured = task.join().unwrap();
        let descendant_stopped = socket.read(&mut [0]).unwrap() == 0;
        let evidence = monitor.finish().unwrap();
        json!({
            "interrupted": captured.is_err_and(|error| error.kind() == std::io::ErrorKind::Interrupted),
            "descendant_stopped": descendant_stopped,
            "evidence": evidence,
        })
    } else {
        let output = process::output_cancellable(
            Command::new("/bin/sh").args(["-c", "printf harmless"]),
            CaptureLimits::default(),
            DEADLINE,
            &monitor::cancelled,
        )
        .unwrap();
        json!({"workload_success": output.status.success(), "evidence": monitor.finish().unwrap()})
    };
    write(&root.join("result.json"), &result.to_string());
}

#[test]
fn workload_helper() {
    if helper_root("workload").is_none() {
        return;
    }
    let mut descendant = helper_command("descendant", "descendant_helper")
        .spawn()
        .unwrap();
    assert!(descendant.wait().unwrap().success());
}

#[test]
fn descendant_helper() {
    let Some(root) = helper_root("descendant") else {
        return;
    };
    let mut socket = UnixStream::connect(root.join("rendezvous.sock")).unwrap();
    socket.set_read_timeout(Some(DEADLINE)).unwrap();
    socket.write_all(b"ready").unwrap();
    let _ = socket.read(&mut [0]);
}
