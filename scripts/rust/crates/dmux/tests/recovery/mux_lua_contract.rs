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
const MUX_START: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../shared/wezterm/mux/dmux-mux-start.sh"
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
    assert!(snapshot.contains("write_recovery_manifest(candidate_id, manifest)"));
    assert!(
        !snapshot.contains("write_json(candidate, manifest)"),
        "snapshot candidates must not bypass the empty-array-preserving writer"
    );
}

#[test]
fn recovery_native_mutations_compare_the_exact_tree_in_the_same_lua_callback() {
    let coordinator = section(
        MUX_LUA,
        "local function run_guarded_recovery",
        "local function schedule_recovery_control",
    );
    assert!(!coordinator.contains("command.action == 'restore_node'"));
    assert!(!coordinator.contains("command.action == 'remove_node'"));

    let restore = section(
        coordinator,
        "command.action == 'compare_and_restore_node'",
        "command.action == 'compare_and_remove_node'",
    );
    assert_ordered(
        restore,
        &[
            "local actual_tree = native_tree_precondition(epoch)",
            "exact_value_equal(actual_tree, command.expected_tree)",
            "context_parent_id(command.node, context)",
            "actual_parent ~= command.expected_parent",
            "resurrect.dmux.execute_restore_node(command.node, context)",
        ],
    );
    let compare = restore
        .find("exact_value_equal(actual_tree, command.expected_tree)")
        .unwrap();
    let create = restore
        .find("resurrect.dmux.execute_restore_node(command.node, context)")
        .unwrap();
    assert!(
        !restore[compare..create].contains("sleep_ms"),
        "raw-tree compare and native create must have no yielding sleep"
    );

    let remove = section(
        coordinator,
        "command.action == 'compare_and_remove_node'",
        "error('unsupported recovery action",
    );
    assert_ordered(
        remove,
        &[
            "local actual_tree = native_tree_precondition(epoch)",
            "exact_value_equal(actual_tree, command.expected_tree)",
            "wezterm.mux.dmux_recovery_remove_node",
        ],
    );
    let compare = remove
        .find("exact_value_equal(actual_tree, command.expected_tree)")
        .unwrap();
    let mutate = remove
        .find("wezterm.mux.dmux_recovery_remove_node")
        .unwrap();
    assert!(
        !remove[compare..mutate].contains("sleep_ms"),
        "raw-tree compare and exact-ID remove must have no yielding sleep"
    );
}

#[test]
fn service_descriptor_identity_comes_only_from_the_native_fixed_path_publisher() {
    let publisher = section(
        MUX_LUA,
        "local function publish_descriptor",
        "local function decode_json",
    );
    assert!(publisher.contains("pcall(wezterm.mux.dmux_publish_service_descriptor, request)"));
    assert!(publisher.contains("local ok, descriptor, raw"));
    assert!(!publisher.contains("io.open"));
    assert!(!publisher.contains("os.rename"));
    assert!(!publisher.contains("request.pid"));
    assert!(!publisher.contains("request.start_token"));
    assert!(!publisher.contains("request.boot_id"));
    assert!(!publisher.contains("request.socket"));

    let coordinator = section(
        MUX_LUA,
        "local function run_guarded_recovery",
        "local function schedule_recovery_control",
    );
    assert!(coordinator.contains("owner_service_descriptor.start_token"));
    assert!(!coordinator.contains("DMUX_START_TOKEN"));

    assert!(MUX_LUA.contains("config.dmux_recovery_primitives = true"));
    assert!(MUX_LUA.contains("config.automatically_reload_config = false"));
    assert!(!MUX_LUA.contains("DMUX_DESCRIPTOR"));
    assert!(!MUX_LUA.contains("DMUX_START_TOKEN"));
    assert!(!MUX_START.contains("DMUX_DESCRIPTOR"));
    assert!(!MUX_START.contains("DMUX_START_TOKEN"));
    assert!(!MUX_START.contains("stub_tmp"));
    assert!(!MUX_START.contains("written_by\":\"wrapper"));
}

#[test]
fn service_config_bootstraps_the_native_fixed_listener_before_mux_startup() {
    assert_ordered(
        MUX_LUA,
        &[
            "pcall(wezterm.mux.dmux_service_bootstrap)",
            "bootstrap.api_version ~= 1",
            "bootstrap.runtime_dir ~= RUNTIME",
            "bootstrap.socket_path ~= SOCK",
            "config.dmux_recovery_primitives = true",
            "config.unix_domains = {",
            "socket_path = SOCK",
            "wezterm.on('mux-startup'",
        ],
    );
    let bootstrap = section(MUX_LUA, "local MISCONFIGURED", "local function now");
    assert!(bootstrap.contains("SOCK = '/tmp/dmux-wez-misconfigured.sock'"));
    assert!(bootstrap.contains("native service bootstrap disagrees with configured fixed paths"));
    assert!(!bootstrap.contains("error("));
    assert!(MUX_START.contains("--dmux-managed-service --config-file"));
    assert!(!MUX_START.contains("mkdir -p \"$runtime\""));
    assert!(!MUX_START.contains("chmod 0700 \"$runtime\""));
}

#[test]
fn recovery_protocol_uses_only_retained_native_storage_capabilities() {
    assert!(MUX_LUA.contains("wezterm.mux.dmux_recovery_spool_open"));
    assert!(MUX_LUA.contains("wezterm.mux.dmux_recovery_manifest_open"));
    assert!(MUX_LUA.contains("owner_recovery_spool:read(kind, MAX_RECOVERY_MESSAGE_BYTES)"));
    assert!(MUX_LUA.contains("owner_recovery_spool:write(kind, raw)"));
    assert!(MUX_LUA.contains("owner_recovery_spool:remove(kind)"));
    assert!(
        MUX_LUA.contains(
            "owner_recovery_manifest:read(candidate_id, kind, MAX_RECOVERY_MANIFEST_BYTES)"
        )
    );
    assert!(MUX_LUA.contains("owner_recovery_manifest:write_candidate(candidate_id"));
    assert!(MUX_LUA.contains("local MAX_RECOVERY_MESSAGE_BYTES = 1024 * 1024"));
    assert!(MUX_LUA.contains("local MAX_RECOVERY_MANIFEST_BYTES = 16 * 1024 * 1024"));

    let coordinator = section(
        MUX_LUA,
        "local function run_guarded_recovery",
        "local function schedule_recovery_control",
    );
    assert!(!coordinator.contains("/recovery/"));
    assert!(!coordinator.contains("command.json"));
    assert!(!coordinator.contains("response.json"));
    assert!(!coordinator.contains("status.json"));
    assert!(!coordinator.contains("--manifest-dir"));
    assert!(!coordinator.contains("DMUX_RECOVERY_MANIFEST_DIR"));
    assert!(!coordinator.contains("io.open"));
    assert!(!coordinator.contains("os.rename"));
    assert!(!coordinator.contains("os.remove"));

    let snapshot = section(
        MUX_LUA,
        "local function publish_guarded_snapshot",
        "schedule_guarded_snapshot = function",
    );
    assert!(snapshot.contains("'--candidate-id'"));
    assert!(snapshot.contains("'--server-epoch'"));
    assert!(snapshot.contains("'--server-start-token'"));
    assert!(!snapshot.contains("'--candidate'"));
    assert!(!snapshot.contains("'--destination'"));
    assert!(!snapshot.contains("DMUX_RECOVERY_MANIFEST_DIR"));
    assert!(!snapshot.contains("io.open"));
    assert!(!snapshot.contains("os.rename"));
    assert!(!snapshot.contains("os.remove"));

    assert!(!MUX_START.contains("DMUX_RECOVERY_MANIFEST_DIR"));
}

#[test]
fn fallback_sentinel_never_publishes_a_ready_descriptor() {
    let startup = section(MUX_LUA, "wezterm.on('mux-startup'", "return config");
    let fallback = section(startup, "if sentinel_fallback then", "if WEZ_FIRST then");
    assert!(fallback.contains("publish_descriptor("));
    assert!(fallback.contains("'failed'"));
    assert!(fallback.contains("return"));
    assert!(!fallback.contains("'ready'"));
}
