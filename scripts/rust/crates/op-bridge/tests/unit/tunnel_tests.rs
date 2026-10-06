use super::*;

fn strings(args: Vec<OsString>) -> Vec<String> {
    args.into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn the_tunnel_forwards_the_remote_socket_to_the_broker() {
    let args = strings(tunnel_args(
        Host::Archie,
        Path::new("/run/user/1000/op-bridge.sock"),
        Path::new("/Users/me/.local/state/op-bridge/broker.sock"),
    ));
    assert_eq!(&args[..2], ["-N", "-T"]);
    let forward = args.iter().position(|arg| arg == "-R").unwrap();
    assert_eq!(
        args[forward + 1],
        "/run/user/1000/op-bridge.sock:/Users/me/.local/state/op-bridge/broker.sock"
    );
    assert_eq!(&args[args.len() - 2..], ["--", "archie"]);
}

#[test]
fn the_tunnel_never_rides_a_shared_control_master() {
    let args = strings(tunnel_args(
        Host::Archie,
        Path::new("/run/user/1000/op-bridge.sock"),
        Path::new("/tmp/broker.sock"),
    ));
    for option in [
        "ControlMaster=no",
        "ControlPath=none",
        "ExitOnForwardFailure=yes",
        "BatchMode=yes",
    ] {
        assert!(args.iter().any(|arg| arg == option), "{option}: {args:?}");
    }
}

#[test]
fn the_prepare_script_prints_the_runtime_socket() {
    let script = prepare_script();
    assert!(
        script.contains("${XDG_RUNTIME_DIR:?}/op-bridge.sock"),
        "{script}"
    );
    assert!(script.contains("rm -f --"), "{script}");
}

#[test]
fn the_remote_socket_must_be_absolute_and_named_for_the_bridge() {
    assert_eq!(
        remote_socket(b"/run/user/1000/op-bridge.sock\n").unwrap(),
        PathBuf::from("/run/user/1000/op-bridge.sock")
    );
    assert!(remote_socket(b"run/user/1000/op-bridge.sock").is_err());
    assert!(remote_socket(b"/run/user/1000/other.sock").is_err());
    assert!(remote_socket(b"/run/user:1000/op-bridge.sock").is_err());
    assert!(remote_socket(b"").is_err());
}
