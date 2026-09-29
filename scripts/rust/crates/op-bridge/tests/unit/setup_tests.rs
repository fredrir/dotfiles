use super::*;

#[test]
fn the_app_and_agent_live_under_home() {
    let paths = Paths::under(Path::new("/Users/me"));
    assert_eq!(paths.app, Path::new("/Users/me/Applications/op-bridge.app"));
    assert_eq!(
        paths.executable(),
        Path::new("/Users/me/Applications/op-bridge.app/Contents/MacOS/op-bridge")
    );
    assert_eq!(
        paths.plist,
        Path::new("/Users/me/Library/LaunchAgents/com.fredrir.op-bridge.plist")
    );
}

#[test]
fn signing_pins_the_bundle_identifier_so_permissions_survive_rebuilds() {
    let args = sign_args(DEFAULT_IDENTITY, Path::new("/tmp/op-bridge.app.new"));
    assert_eq!(
        args,
        [
            "--force",
            "--sign",
            "Developer ID Application",
            "--identifier",
            "com.fredrir.op-bridge",
            "--timestamp=none",
            "/tmp/op-bridge.app.new",
        ]
    );
}

#[test]
fn the_info_plist_names_the_same_bundle_and_executable() {
    assert!(INFO_PLIST.contains("<string>com.fredrir.op-bridge</string>"));
    assert!(INFO_PLIST.contains("<key>CFBundleExecutable</key>\n  <string>op-bridge</string>"));
    assert!(EXECUTABLE.ends_with("/op-bridge"));
}

#[test]
fn the_running_program_is_read_from_launchctl_print() {
    let print = "gui/501/com.fredrir.op-bridge = {\n\tactive count = 1\n\tprogram = /Users/me/Applications/op-bridge.app/Contents/MacOS/op-bridge\n\tpid = 42\n}";
    assert_eq!(
        program(print),
        Some(Path::new(
            "/Users/me/Applications/op-bridge.app/Contents/MacOS/op-bridge"
        ))
    );
    assert_eq!(program("state = running"), None);
}
