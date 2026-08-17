package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

local act = setmetatable({ PopKeyTable = { name = 'PopKeyTable' } }, {
  __index = function(_, name)
    return function(value)
      value = value or {}
      value.name = name
      return value
    end
  end,
})
local fake_wezterm = {
  action = act,
  action_callback = function(callback)
    return { name = 'Callback', callback = callback }
  end,
}
package.preload.wezterm = function()
  return fake_wezterm
end

local group_count = 2
local group_ref = 'gepoch.wz-7'
local split_ref = 'pepoch.wz-9'
local calls = {}
local toasts = {}
package.loaded['wez.dmux_bridge.controller'] = {
  run = function(_, _, verb, args)
    table.insert(calls, { verb = verb, args = args })
    if verb == 'context' then
      return {
        marker = { group_ref = group_ref, split_ref = split_ref },
        display = {
          logical_ref = 'b2',
          space_name = 'project',
          group_name = 'editor',
          group_count = group_count,
        },
      }
    end
    return {}
  end,
  toast = function(_, message)
    table.insert(toasts, message)
  end,
}

local actions = require 'wez.dmux_bridge.actions'
local function binding(key, mods)
  for _, item in ipairs(actions.keys()) do
    if item.key == key and item.mods == mods then
      return item.action
    end
  end
  error('binding not found: ' .. mods .. '+' .. key)
end

local performed = {}
local window = {
  perform_action = function(_, action, pane)
    table.insert(performed, { action = action, pane = pane })
  end,
}
local pane = {}

local client_uid = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'
local uid_pane = {
  get_user_vars = function()
    return { dmux_tmux_client_uid = client_uid }
  end,
}
local correlated = actions.with_tmux_client_uid(uid_pane, { '--name', 'scratch' })
assert(correlated[1] == '--name' and correlated[2] == 'scratch')
assert(correlated[3] == '--tmux-client-uid' and correlated[4] == client_uid)
local malformed = actions.with_tmux_client_uid {
  get_user_vars = function()
    return { dmux_tmux_client_uid = string.upper(client_uid) }
  end,
}
assert(#malformed == 0, 'noncanonical client UIDs must not cross the Lua/Rust boundary')

local close_group = binding('w', 'CTRL')
close_group.callback(window, pane)
local selector = performed[#performed].action
assert(selector.name == 'InputSelector')
assert(selector.title:find('editor', 1, true) and selector.title:find('project', 1, true))
assert(selector.title:find('gepoch.wz-7', 1, true))
selector.action.callback(window, pane, 'cancel')
assert(#calls == 1, 'canceling Group removal must not mutate')

group_ref = 'gepoch.wz-8'
selector.action.callback(window, pane, 'remove')
assert(calls[#calls].verb == 'context' and #toasts == 1, 'changed Group context must fail before removal')
group_ref = 'gepoch.wz-7'
close_group.callback(window, pane)
selector = performed[#performed].action
selector.action.callback(window, pane, 'remove')
assert(calls[#calls].verb == 'group-remove')
assert(calls[#calls].args[1] == '--confirmed' and calls[#calls].args[2] == nil)

group_count = 1
close_group.callback(window, pane)
selector = performed[#performed].action
selector.action.callback(window, pane, 'remove')
local escalation = performed[#performed].action
assert(escalation.name == 'InputSelector')
assert(escalation.title:find('final Group', 1, true) and escalation.title:find('project', 1, true))
local before_cancel = #calls
escalation.action.callback(window, pane, 'cancel')
assert(#calls == before_cancel, 'canceling final-Space escalation must not mutate')
escalation.action.callback(window, pane, 'remove-space')
assert(calls[#calls].verb == 'group-remove')
assert(calls[#calls].args[1] == '--confirmed' and calls[#calls].args[2] == '--escalate-space')

local close_split = binding('x', 'LEADER')
close_split.callback(window, pane)
selector = performed[#performed].action
assert(selector.title:find('pepoch.wz-9', 1, true))
local before_split_cancel = #calls
selector.action.callback(window, pane, 'cancel')
assert(#calls == before_split_cancel, 'canceling Split removal must not mutate')
split_ref = 'pepoch.wz-10'
selector.action.callback(window, pane, 'remove')
assert(calls[#calls].verb == 'context' and #toasts == 2, 'changed Split context must fail before removal')
split_ref = 'pepoch.wz-9'
close_split.callback(window, pane)
selector = performed[#performed].action
selector.action.callback(window, pane, 'remove')
assert(calls[#calls].verb == 'split-remove' and calls[#calls].args[1] == '--confirmed')

io.stdout:write 'dmux action test: exact client UID/cancel/confirm/final-Group escalation passed\n'
