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
local calls, toasts, reports = {}, {}, {}
local malformed
package.loaded['wez.dmux_bridge.controller'] = {
  run = function(_, _, verb, args)
    table.insert(calls, { verb = verb, args = args })
    if verb == 'spaces' then
      return malformed or { spaces = rows }
    end
    return {}
  end,
  toast = function(_, message)
    table.insert(toasts, message)
  end,
  report = function(_, verb, message, code)
    table.insert(reports, { verb = verb, message = message, code = code })
  end,
}

local picker = require 'wez.plugins.workspace_picker'
local window = {
  perform_action = function(_, action)
    selector = action
  end,
  toast_notification = function() end,
}
local client_uid
local pane = {
  get_user_vars = function()
    return { dmux_tmux_client_uid = client_uid }
  end,
}

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

-- Tmux rows are accepted only after Rust has preflighted this pane's exact
-- attach-time client UID; selection sends the same UID back for switching.
client_uid = '11111111-1111-4111-8111-111111111111'
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
assert(selector and #selector.choices == 3)
assert(selector.choices[3].id == 'a1' and selector.choices[3].label:match 'macie/tmux')
selector.action(window, pane, 'a1')
assert(calls[#calls].verb == 'present')
assert(calls[#calls].args[1] == '--space' and calls[#calls].args[2] == 'a1')
assert(calls[#calls].args[3] == '--tmux-client-uid' and calls[#calls].args[4] == client_uid)

-- A malformed controller response is a defect, not a benign abort: it must
-- reach the log and never build a selector from unvalidated rows.
for _, case in ipairs {
  { result = { spaces = 'not-a-table' }, message = 'malformed Space picker result' },
  { result = { spaces = json.array {}, extra = true }, message = 'malformed Space picker result' },
  { result = { spaces = json.array { { ref = 'b2' } } }, message = 'malformed Space picker rows' },
} do
  malformed = case.result
  selector = nil
  reports = {}
  picker.action()(window, pane)
  assert(selector == nil, 'a malformed picker response must not open a selector')
  assert(#reports == 1 and reports[1].verb == 'spaces', 'a malformed picker response must be reported')
  assert(reports[1].message:match(case.message), reports[1].message)
end
malformed = nil

io.stdout:write 'dmux picker test: named Wez Spaces and exact-client tmux rows passed\n'
