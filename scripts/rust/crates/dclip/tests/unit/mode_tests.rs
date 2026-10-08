use std::collections::HashMap;

use hostkit::Route;

use super::*;

fn mode(this: Host, native: bool, pairs: &[(&str, &str)]) -> Mode {
    let env: HashMap<String, OsString> = pairs
        .iter()
        .map(|(name, value)| (name.to_string(), OsString::from(value)))
        .collect();
    detect(this, native, |name| env.get(name).cloned())
}

#[test]
fn a_mux_pane_pastes_from_its_origin_even_beside_a_native_clipboard() {
    let found = mode(
        Host::Macie,
        true,
        &[
            ("HWIRE_SESSION", "v1:archie:macie:tailscale:tls"),
            ("SSH_CONNECTION", "1 2 3 4"),
        ],
    );
    assert_eq!(
        found,
        Mode::Mux(Stamp {
            origin: Host::Archie,
            destination: Host::Macie,
            route: Route::Tailscale
        })
    );
}

#[test]
fn a_stamp_for_another_host_is_ignored() {
    let stamp = [("HWIRE_SESSION", "v1:macie:archie:cable:tls")];
    assert_eq!(mode(Host::Macie, true, &stamp), Mode::Native);
    assert_eq!(mode(Host::Macie, false, &stamp), Mode::Terminal);
}

#[test]
fn ssh_comes_before_a_native_clipboard() {
    assert_eq!(
        mode(Host::Archie, true, &[("SSH_TTY", "/dev/pts/3")]),
        Mode::Ssh
    );
    assert_eq!(
        mode(Host::Macie, true, &[("SSH_CONNECTION", "1 2 3 4")]),
        Mode::Ssh
    );
}

#[test]
fn empty_variables_count_as_unset() {
    let empty = [
        ("HWIRE_SESSION", ""),
        ("SSH_CONNECTION", ""),
        ("SSH_TTY", ""),
    ];
    assert_eq!(mode(Host::Archie, true, &empty), Mode::Native);
    assert_eq!(mode(Host::Archie, false, &empty), Mode::Terminal);
}
