use super::*;
use std::net::SocketAddr;

use crate::forward::Bind;

fn strings(args: Vec<OsString>) -> Vec<String> {
    args.into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn the_master_uses_the_peer_alias_so_ssh_config_picks_the_route() {
    let args = strings(master_args(Path::new("/tmp/m.sock"), Host::Archie));
    assert_eq!(&args[..5], ["-M", "-N", "-T", "-S", "/tmp/m.sock"]);
    assert_eq!(&args[args.len() - 2..], ["--", "archie"]);
    assert!(args.iter().any(|arg| arg == "BatchMode=yes"));
    assert!(args.iter().any(|arg| arg == "ControlPersist=no"));
}

#[test]
fn control_commands_skip_ssh_config_and_name_an_unresolvable_host() {
    let target: SocketAddr = "127.0.0.1:5173".parse().unwrap();
    let forward = Forward {
        bind: Bind::Alias,
        port: 5173,
        target,
    };
    assert_eq!(
        strings(control_args(
            Path::new("/tmp/m.sock"),
            "forward",
            Some(&forward)
        )),
        [
            "-F",
            "none",
            "-S",
            "/tmp/m.sock",
            "-O",
            "forward",
            "-L",
            "127.0.0.2:5173:127.0.0.1:5173",
            "hport.invalid"
        ]
    );
    assert_eq!(
        strings(control_args(Path::new("/tmp/m.sock"), "check", None)),
        [
            "-F",
            "none",
            "-S",
            "/tmp/m.sock",
            "-O",
            "check",
            "hport.invalid"
        ]
    );
}

#[test]
fn a_session_runs_its_script_through_the_master() {
    let args = strings(session_args(Path::new("/tmp/m.sock"), "true"));
    assert_eq!(&args[..2], ["-F", "none"]);
    assert_eq!(&args[args.len() - 2..], ["hport.invalid", "true"]);
    assert!(args.windows(2).any(|pair| pair == ["-S", "/tmp/m.sock"]));
}
