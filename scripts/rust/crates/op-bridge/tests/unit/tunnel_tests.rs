use super::*;

fn strings(args: Vec<OsString>) -> Vec<String> {
    args.into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

fn args() -> Vec<String> {
    strings(tunnel_args(
        Host::Archie,
        Path::new("/run/user/1000/op-bridge-tunnel.sock"),
        Path::new("/Users/me/.local/state/op-bridge/broker.sock"),
        Path::new("/Users/me/.local/state/op-bridge/archie.sock"),
    ))
}

fn after(args: &[String], flag: &str) -> String {
    let position = args.iter().position(|arg| arg == flag).unwrap();
    args[position + 1].clone()
}

#[test]
fn the_tunnel_forwards_archies_tunnel_socket_to_the_broker() {
    let args = args();
    assert_eq!(&args[..2], ["-N", "-T"]);
    assert_eq!(
        after(&args, "-R"),
        "/run/user/1000/op-bridge-tunnel.sock:/Users/me/.local/state/op-bridge/broker.sock"
    );
    assert_eq!(&args[args.len() - 2..], ["--", "archie"]);
}

#[test]
fn the_tunnel_forwards_macies_reloads_to_archies_daemon() {
    assert_eq!(
        after(&args(), "-L"),
        "/Users/me/.local/state/op-bridge/archie.sock:/run/user/1000/op-bridge.sock"
    );
}

#[test]
fn the_tunnel_never_rides_a_shared_control_master() {
    let args = args();
    for option in [
        "ControlMaster=no",
        "ControlPath=none",
        "ExitOnForwardFailure=yes",
        "StreamLocalBindUnlink=yes",
        "BatchMode=yes",
    ] {
        assert!(args.iter().any(|arg| arg == option), "{option}: {args:?}");
    }
}

#[test]
fn the_prepare_script_prints_the_runtime_socket() {
    let script = prepare_script();
    assert!(
        script.contains("${XDG_RUNTIME_DIR:?}/op-bridge-tunnel.sock"),
        "{script}"
    );
    assert!(script.contains("rm -f --"), "{script}");
}

#[test]
fn the_remote_socket_must_be_absolute_and_named_for_the_tunnel() {
    assert_eq!(
        remote_socket(b"/run/user/1000/op-bridge-tunnel.sock\n").unwrap(),
        PathBuf::from("/run/user/1000/op-bridge-tunnel.sock")
    );
    assert!(remote_socket(b"run/user/1000/op-bridge-tunnel.sock").is_err());
    assert!(remote_socket(b"/run/user/1000/op-bridge.sock").is_err());
    assert!(remote_socket(b"/run/user:1000/op-bridge-tunnel.sock").is_err());
    assert!(remote_socket(b"").is_err());
}
