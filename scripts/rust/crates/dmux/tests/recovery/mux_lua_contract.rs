//! Source-level owner-Lua contract for the recovery helper wire.
//!
//! The mux config cannot be required as a library without starting its
//! `mux-startup` event graph.  These narrow assertions instead pin the two
//! security-sensitive launch sites and the empty-manifest encoding shim in
//! the exact config that the service loads.

const MUX_LUA: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../shared/wezterm/mux/dmux-mux.lua"
));

fn section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start = source
        .find(start)
        .unwrap_or_else(|| panic!("missing Lua section start {start:?}"));
    let tail = &source[start..];
    let end = tail
        .find(end)
        .unwrap_or_else(|| panic!("missing Lua section end {end:?}"));
    &tail[..end]
}

fn assert_ordered(haystack: &str, needles: &[&str]) {
    let mut cursor = 0;
    for needle in needles {
        let relative = haystack[cursor..]
            .find(needle)
            .unwrap_or_else(|| panic!("missing ordered Lua token {needle:?}"));
        cursor += relative + needle.len();
    }
}

#[test]
fn coordinator_and_snapshot_helpers_scrub_all_ambient_mux_identity() {
    let scrubber = section(
        MUX_LUA,
        "local function registry_only_argv",
        "local schedule_guarded_snapshot",
    );
    assert_ordered(
        scrubber,
        &[
            "'/usr/bin/env'",
            "'-u'",
            "'WEZTERM_UNIX_SOCKET'",
            "'-u'",
            "'WEZTERM_PANE'",
            "'-u'",
            "'TMUX'",
            "'-u'",
            "'TMUX_PANE'",
        ],
    );

    let coordinator = section(
        MUX_LUA,
        "local function run_guarded_recovery",
        "local function schedule_recovery_control",
    );
    assert_eq!(
        coordinator
            .matches("wezterm.background_child_process")
            .count(),
        1,
        "the coordinator section must have one auditable helper launch"
    );
    assert!(
        coordinator.contains("pcall(wezterm.background_child_process, registry_only_argv(argv))"),
        "the coordinator helper must receive only registry-only argv"
    );

    let snapshot = section(
        MUX_LUA,
        "local function publish_guarded_snapshot",
        "schedule_guarded_snapshot = function",
    );
    assert_eq!(
        snapshot.matches("wezterm.background_child_process").count(),
        1,
        "the snapshot section must have one auditable helper launch"
    );
    let launch = snapshot
        .find("wezterm.background_child_process")
        .expect("snapshot helper launch exists");
    assert!(
        snapshot[launch..]
            .starts_with("wezterm.background_child_process,\n    registry_only_argv {"),
        "the snapshot helper must receive only registry-only argv"
    );
}

#[test]
fn empty_snapshot_spaces_are_serialized_as_a_json_array() {
    let writer = section(
        MUX_LUA,
        "local function write_recovery_manifest",
        "local function native_snapshot",
    );
    assert!(writer.contains("type(manifest.spaces) == 'table'"));
    assert!(writer.contains("next(manifest.spaces) == nil"));
    assert!(writer.contains("encoded:gsub('\"spaces\":{}', '\"spaces\":[]', 1)"));
    assert!(writer.contains("replacements ~= 1"));

    let snapshot = section(
        MUX_LUA,
        "local function publish_guarded_snapshot",
        "schedule_guarded_snapshot = function",
    );
    assert!(snapshot.contains("write_recovery_manifest(candidate, manifest)"));
    assert!(
        !snapshot.contains("write_json(candidate, manifest)"),
        "snapshot candidates must not bypass the empty-array-preserving writer"
    );
}
