use super::*;
use std::net::SocketAddr;

fn service(port: u16, process: &str) -> Service {
    Service {
        port,
        process: process.into(),
        target: SocketAddr::from(([127, 0, 0, 1], port)),
    }
}

#[test]
fn an_empty_file_keeps_the_defaults() {
    assert_eq!(Config::parse("").unwrap(), Config::default());
    assert_eq!(Config::default().max_port, 32767);
}

#[test]
fn every_key_is_read() {
    let config = Config::parse(
        r#"
max_port = 20000
ignore_ports = [8443, 8446]
ignore_processes = ["ControlCenter"]
"#,
    )
    .unwrap();
    assert_eq!(config.max_port, 20000);
    assert!(config.ignore_ports.contains(&8446));
    assert!(config.ignore_processes.contains("ControlCenter"));
}

#[test]
fn a_misspelled_key_is_an_error_rather_than_silently_ignored() {
    assert!(Config::parse("ignore_port = [1]").is_err());
}

#[test]
fn ports_in_the_ephemeral_range_are_left_alone() {
    let config = Config::default();
    assert!(config.admits(&service(5173, "bun")));
    assert!(config.admits(&service(32767, "bun")));
    assert!(!config.admits(&service(44093, "java")));
}

#[test]
fn ignored_ports_and_processes_are_left_alone() {
    let config =
        Config::parse("ignore_ports = [8443]\nignore_processes = [\"ControlCenter\"]").unwrap();
    assert!(!config.admits(&service(8443, "wezterm-mux-server")));
    assert!(!config.admits(&service(5000, "ControlCenter")));
    assert!(config.admits(&service(5000, "python")));
}
