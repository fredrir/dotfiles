#!/bin/sh
set -eu

source_root=${DMUX_WEZTERM_SOURCE:?set DMUX_WEZTERM_SOURCE to the maintained WezTerm fork checkout}

require_text() {
  pattern=$1
  file=$2
  if ! rg -q -F "$pattern" "$source_root/$file"; then
    echo "missing managed-GUI source invariant: $pattern ($file)" >&2
    exit 1
  fi
}

reject_text() {
  pattern=$1
  file=$2
  if rg -q -F "$pattern" "$source_root/$file"; then
    echo "forbidden managed-GUI source surface remains: $pattern ($file)" >&2
    exit 1
  fi
}

require_tree_count() {
  expected=$1
  pattern=$2
  actual=$(rg -F -c --no-filename -g '*.rs' "$pattern" "$source_root" | awk '{ total += $1 } END { print total + 0 }')
  if [ "$actual" != "$expected" ]; then
    echo "managed-GUI source invariant moved: expected $expected occurrences of $pattern, found $actual" >&2
    exit 1
  fi
}

require_text 'pub dmux_managed_gui: bool' config/src/config.rs
require_text 'should_expose_ui_action(config.dmux_managed_gui, &cmd.action)' wezterm-gui/src/commands.rs
require_text 'should_expose_ui_action(config.dmux_managed_gui, &entry.action)' wezterm-gui/src/overlay/launcher.rs
require_text 'should_expose_ui_action(dmux_managed_gui, &cmd.action)' wezterm-gui/src/termwindow/palette.rs
require_text 'dmux-managed-window-close-requested' wezterm-gui/src/termwindow/mod.rs
require_text 'RefuseAndNotifyBroker' wezterm-gui/src/termwindow/mod.rs
require_text 'pub(crate) fn should_perform_native_action' wezterm-gui/src/dmux_managed.rs
require_text '| HideApplication' wezterm-gui/src/dmux_managed.rs
require_text 'should_perform_native_action(' wezterm-gui/src/termwindow/mod.rs
require_text 'should_perform_native_action(' wezterm-gui/src/frontend.rs
require_text 'pub(crate) fn should_close_tab_directly' wezterm-gui/src/dmux_managed.rs
require_text 'should_close_tab_directly(self.config.dmux_managed_gui)' wezterm-gui/src/termwindow/mouseevent.rs
require_text 'refusing native application termination for dmux-managed GUI' window/src/os/macos/app.rs
require_text 'ignoring native new-window request for dmux-managed GUI' window/src/os/macos/app.rs
require_text 'return dock_menu.autorelease()' window/src/os/macos/app.rs

# Config load errors normally retain stock defaults, so managed GUI startup
# must reject the process before build_initial_mux and accept attach-only
# broker invocations. Direct app/Dock starts cannot fall through to a shell.
require_text 'pub(crate) fn require_successful_managed_config_load' wezterm-gui/src/dmux_managed.rs
require_text 'pub(crate) fn require_managed_startup_contract' wezterm-gui/src/dmux_managed.rs
require_text 'require_dmux_managed_gui_startup(&sub)?;' wezterm-gui/src/main.rs
require_text 'require_existing_panes_after_managed_attach(' wezterm-gui/src/main.rs

# The service socket is prebound while the process is still single-threaded;
# config must claim that exact retained listener, and a failed sentinel may
# never fall through to WezTerm's ordinary default-program spawn.
require_text 'long = "dmux-managed-service"' wezterm-mux-server/src/main.rs
require_text 'let managed_flag_present = dmux_managed_service_flag_present(&args);' wezterm-mux-server/src/main.rs
require_text 'prebind_dmux_managed_service()' wezterm-mux-server/src/main.rs
require_text 'bootstrap.validate_and_take(&config)?' wezterm-mux-server/src/main.rs
require_text 'dmux managed mux-startup produced no sentinel/user pane' wezterm-mux-server/src/main.rs
require_text 'pub fn prebind_dmux_managed_service()' lua-api-crates/mux/src/dmux_descriptor.rs

# Lifecycle completion is available only on the exclusive retained bridge
# capability. It consumes one exact authenticated request+ack proof before
# application-scoped hide/quit. No global or window method may bypass that
# proof, and native QuitApplication remains runtime-denied in managed mode.
require_text 'fn complete_safe_lifecycle(&self, uid: &str, platform_action: &str)' wezterm-gui/src/scripting/dmux_bridge.rs
require_text 'self.consume_lifecycle_completion_proof(uid, platform_action)?;' wezterm-gui/src/scripting/dmux_bridge.rs
require_text '"complete_safe_lifecycle",' wezterm-gui/src/scripting/dmux_bridge.rs
require_text 'connection.terminate_message_loop();' wezterm-gui/src/scripting/dmux_bridge.rs
require_text 'managed_lifecycle_completion_is_not_globally_registered' wezterm-gui/src/scripting/mod.rs
reject_text 'methods.add_async_method("dmux_safe_quit_application"' wezterm-gui/src/scripting/guiwin.rs
reject_text 'methods.add_async_method("dmux_safe_hide_application"' wezterm-gui/src/scripting/guiwin.rs
reject_text 'pub(crate) fn dmux_safe_quit_application' wezterm-gui/src/termwindow/mod.rs
reject_text 'pub(crate) fn dmux_safe_hide_application' wezterm-gui/src/termwindow/mod.rs

# Both GUI-side domain inventories exempt WezTerm's connection-UI placeholder
# by capability rather than by name, because the mux leaks one per attach and
# never frees it. That is an exact discriminator only while the trait default
# is the sole other implementation: a second one returning false would silently
# exempt a domain dmux must keep policing, so pin the count, not just the name.
require_text '"is_spawnable"' lua-api-crates/mux/src/domain.rs
require_tree_count 2 'fn spawnable(&self) -> bool {'

# A retained bridge/recovery capability cannot cross Lua generations.
require_text 'self.config.dmux_managed_gui || self.config.dmux_recovery_primitives' config/src/lib.rs
require_text '| ReloadConfiguration => true' wezterm-gui/src/dmux_managed.rs

echo 'dmux maintained-fork surface test: managed UI gates present'
