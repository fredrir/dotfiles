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

# The signed bridge uses a narrow, non-KeyAssignment completion method only
# after its ack has been durably published. Native QuitApplication remains
# runtime-denied in managed mode.
require_text 'methods.add_async_method("dmux_safe_quit_application"' wezterm-gui/src/scripting/guiwin.rs
require_text 'methods.add_async_method("dmux_safe_hide_application"' wezterm-gui/src/scripting/guiwin.rs
require_text 'pub(crate) fn dmux_safe_quit_application' wezterm-gui/src/termwindow/mod.rs
require_text 'pub(crate) fn dmux_safe_hide_application' wezterm-gui/src/termwindow/mod.rs
require_text 'require_managed_safe_quit_api(self.config.dmux_managed_gui)?;' wezterm-gui/src/termwindow/mod.rs
require_text 'require_managed_safe_hide_api(self.config.dmux_managed_gui)?;' wezterm-gui/src/termwindow/mod.rs
require_text 'con.terminate_message_loop();' wezterm-gui/src/termwindow/mod.rs

echo 'dmux maintained-fork surface test: managed UI gates present'
