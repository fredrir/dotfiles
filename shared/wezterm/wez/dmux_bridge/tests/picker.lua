package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

assert(os.getenv 'DMUX_WEZ_FIRST' == '1')

local json = require 'wez.dmux_bridge.json'

local selector
local fake_wezterm = {
  action = {
    InputSelector = function(spec)
      return spec
    end,
  },
  action_callback = function(callback)
    return callback
  end,
}
package.preload.wezterm = function()
  return fake_wezterm
end
package.preload['wez.platform'] = function()
  return {
    pick = function(value)
      return value.mac
    end,
  }
end

local rows = json.array {
  {
    ref = 'b2',
    name = 'dotfiles',
    backend = 'wez',
    owner_alias = 'b',
    owner_label = 'Archie',
    route = 'tailscale',
    attached = true,
    health = 'healthy',
  },
}
local calls, toasts = {}, {}
package.loaded['wez.dmux_bridge.controller'] = {
  run = function(_, _, verb, args)
    table.insert(calls, { verb = verb, args = args })
    if verb == 'spaces' then
      return { spaces = rows }
    end
    return {}
  end,
  toast = function(_, message)
    table.insert(toasts, message)
  end,
}

local picker = require 'wez.plugins.workspace_picker'
local window = {
  perform_action = function(_, action)
    selector = action
  end,
  toast_notification = function() end,
}
local pane = {}

picker.action()(window, pane)
assert(selector and selector.title == 'dmux Spaces' and #selector.choices == 3)
assert(selector.choices[3].id == 'b2')
assert(selector.choices[3].label:match 'dotfiles')
assert(selector.choices[3].label:match 'Archie/wez')
assert(selector.choices[3].label:match 'tailscale')
assert(selector.choices[3].label:match 'attached')
selector.action(window, pane, 'b2')
assert(calls[#calls].verb == 'present')
assert(calls[#calls].args[1] == '--space' and calls[#calls].args[2] == 'b2')

-- A controller regression that leaks a tmux row must not expose an item
-- whose presentation cannot be completed by this GUI bridge.
rows = json.array {
  {
    ref = 'a1',
    name = 'tmux-only',
    backend = 'tmux',
    owner_alias = 'a',
    owner_label = 'macie',
    route = 'local',
    attached = false,
    health = 'healthy',
  },
}
selector = nil
picker.action()(window, pane)
assert(selector == nil)
assert(toasts[#toasts] == 'dmux returned malformed Space picker rows')

io.stdout:write 'dmux picker test: named Wez Spaces and tmux fail-closed rows passed\n'
