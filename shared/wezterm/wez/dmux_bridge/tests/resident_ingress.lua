package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

assert(os.getenv 'DMUX_WEZ_FIRST' == '1')

local events = {}
local log_errors = {}
local resident_calls = {}
local resident_failure

local fake_wezterm = {
  GLOBAL = {},
  gui = {},
  log_error = function(message)
    table.insert(log_errors, message)
  end,
  mux = {
    -- The application has no user window or pane. The service's reserved
    -- sentinel is deliberately not usable as a GUI action origin.
    all_windows = function()
      return { { workspace = 'dmux:system:sentinel-only' } }
    end,
  },
  on = function(name, callback)
    events[name] = callback
  end,
}
package.preload.wezterm = function()
  return fake_wezterm
end

package.loaded['wez.dmux_bridge.controller'] = {
  run = function()
    error 'marker-bound controller path must not run for a zero-window application quit'
  end,
  run_resident = function(verb, args)
    table.insert(resident_calls, { verb = verb, args = args })
    if resident_failure then
      return nil, resident_failure
    end
    return { quit = true }
  end,
}
package.loaded['wez.dmux_bridge.consumer'] = {
  start = function()
    return true
  end,
}

local bridge = require 'wez.dmux_bridge'
bridge.setup()

local application_quit = events['dmux-managed-application-quit-requested']
assert(type(application_quit) == 'function', 'zero-window maintained-fork ingress was not registered')

application_quit()
assert(#resident_calls == 1)
assert(resident_calls[1].verb == 'safe-quit' and resident_calls[1].args == nil)
assert(fake_wezterm.gui.dmux_safe_quit_application == nil and fake_wezterm.gui.dmux_safe_hide_application == nil)

resident_failure = 'resident proof failed'
application_quit()
assert(#resident_calls == 2)
assert(#log_errors == 1 and log_errors[1]:match 'failed closed: resident proof failed')
assert(fake_wezterm.gui.dmux_safe_quit_application == nil and fake_wezterm.gui.dmux_safe_hide_application == nil)
assert(fake_wezterm.GLOBAL.dmux_managed_close_in_progress == false, 'resident ingress guard was not released')

io.stdout:write 'dmux resident ingress test: markerless sentinel-only safe quit fails closed\n'
