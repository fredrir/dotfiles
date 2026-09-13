-- Run from the repository root: lua shared/wezterm/tests/tmux-workspace.lua linux|mac
local is_mac = arg[1] == "mac"
package.path = "shared/wezterm/?.lua;shared/wezterm/?/init.lua;" .. package.path

local events = {}
local action = setmetatable({}, {
  __index = function(self, name)
    local value = setmetatable({ name = name }, {
      __call = function(_, args)
        return { name = name, args = args }
      end,
    })
    rawset(self, name, value)
    return value
  end,
})
package.preload.wezterm = function()
  return {
    action = action,
    action_callback = function(callback)
      return { callback = callback }
    end,
    -- WezTerm runs every registered handler; a single slot would let the last
    -- module to subscribe silently unsubscribe the others.
    on = function(name, callback)
      local previous = events[name]
      events[name] = previous
          and function(...)
            previous(...)
            return callback(...)
          end
        or callback
    end,
  }
end
package.preload["utils.platform"] = function()
  return { is_mac = is_mac }
end
for _, name in ipairs {
  "utils.close-tab",
  "utils.close-pane",
  "utils.attach-remote",
  "utils.open-vscode",
  "ui.skip_close_confirmation",
  "keymap.mouse-bindings",
} do
  package.preload[name] = function()
    return { name = name }
  end
end
package.preload["utils.resize-window"] = function()
  return function()
    return action.Nop
  end
end
package.preload["utils.hwire-session"] = function()
  if arg[2] == "native-splits" then
    return { new_tab = action.SpawnTab }
  end
  return {
    new_tab = action.SpawnTab,
    split = function(direction)
      return { name = "split", direction = direction }
    end,
  }
end
package.preload["utils.mux"] = function()
  return { detach_pane = action.Nop, attach_detached = action.Nop }
end

local keys = require("keymap").keys
local by_key = {}
for _, binding in ipairs(keys) do
  local modifiers = {}
  for modifier in binding.mods:gmatch "[^|]+" do
    table.insert(modifiers, modifier)
  end
  table.sort(modifiers)
  local id = table.concat(modifiers, "|") .. ":" .. binding.key
  assert(not by_key[id], "duplicate binding " .. id)
  by_key[id] = binding.action
end
local function binding(key, mods)
  local modifiers = {}
  for modifier in mods:gmatch "[^|]+" do
    table.insert(modifiers, modifier)
  end
  table.sort(modifiers)
  return by_key[table.concat(modifiers, "|") .. ":" .. key]
end
local primary = is_mac and "CMD" or "CTRL"
local unique = is_mac and "OPT" or "ALT"
local vars = {}
local pane = {
  pane_id = function()
    return 7
  end,
  get_user_vars = function()
    return vars
  end,
}
local performed
local window = {
  perform_action = function(_, value)
    performed = value
  end,
  toast_notification = function() end,
}
local tmux = require "utils.tmux-workspace"

assert(not tmux.active(pane), "missing lifecycle marker must not claim tmux")
binding("t", primary).callback(window, pane)
assert(performed.name == "SpawnTab")
binding("d", primary).callback(window, pane)
assert(performed.name == (arg[2] == "native-splits" and "SplitHorizontal" or "split"))
vars.TMUX = "inherited-but-not-attached"
assert(not tmux.active(pane), "ancestry must not claim tmux")
vars.TMUX_WORKSPACE = "1"
for key, expected in pairs { t = "t", d = "d", w = "w", q = "q", ["0"] = "0", ["9"] = "9", ["."] = "." } do
  binding(key, primary).callback(window, pane)
  assert(performed.name == "Multiple")
  assert(performed.args[1].args.key == "b" and performed.args[1].args.mods == "CTRL")
  assert(performed.args[2].args.key == expected)
end
binding("Tab", "CTRL|SHIFT").callback(window, pane)
assert(performed.args[2].args.key == "p")
binding(is_mac and "'" or "§", primary).callback(window, pane)
assert(performed.args[2].args.key == "'")
binding("s", primary .. "|SHIFT").callback(window, pane)
assert(performed.args[2].args.key == "S")
binding("m", primary .. "|SHIFT").callback(window, pane)
assert(performed.args[2].args.key == "M")

assert(binding("Enter", "SHIFT").args == "\x1b[13;2u")
assert(binding("Enter", primary).args == "\x05\x1b[13;2u")
binding("Enter", primary .. "|SHIFT").callback(window, pane)
assert(performed.args == "\x01\x1b[13;2u\x02\x02", "tmux must forward the cursor-left prefix")
binding("y", primary).callback(window, pane)
assert(performed.args == "\x1b[5;30012~", "tmux requires the reserved widget transport")
assert(binding("Backspace", primary).args == "\x15")
assert(binding("Backspace", primary .. "|SHIFT").args == "\x0b")

-- Document and selection motions reach the shell verbatim, so they survive
-- tmux and still mean the same thing to an editor running in the pane.
assert(binding("UpArrow", primary).args == "\x1b[1;5H")
assert(binding("DownArrow", primary).args == "\x1b[1;5F")
assert(binding("LeftArrow", primary .. "|SHIFT").args == "\x1b[1;2H")
assert(binding("RightArrow", primary .. "|SHIFT").args == "\x1b[1;2F")
assert(binding("UpArrow", primary .. "|SHIFT").args == "\x1b[1;6H")
assert(binding("DownArrow", primary .. "|SHIFT").args == "\x1b[1;6F")
binding("UpArrow", unique).callback(window, pane)
assert(performed.args[2].args.key == "Up", "prompt jumping moved to the unique modifier")
assert(not binding("h", "CTRL"), "Atuin owns Ctrl-h in the shell")
if is_mac then
  assert(binding("phys:8", "OPT").args == "[")
  assert(binding("phys:9", "OPT|SHIFT").args == "}")
else
  assert(not binding("c", "CTRL"), "Ctrl-c must interrupt the shell")
  assert(not binding("v", "CTRL"), "Ctrl-v must quote input in the shell")
  assert(binding("c", "CTRL|SHIFT").name == "CopyTo")
  assert(binding("v", "CTRL|SHIFT").name == "PasteFrom")
end

tmux.toggle.callback(window, pane)
assert(not tmux.active(pane))
tmux.toggle.callback(window, pane)
assert(tmux.active(pane))
tmux.toggle.callback(window, pane)
events["user-var-changed"](window, pane, "TMUX_WORKSPACE", "1")
assert(tmux.active(pane), "lifecycle updates must clear manual overrides")
vars.TMUX_WORKSPACE = ""
events["user-var-changed"](window, pane, "TMUX_WORKSPACE", "")
assert(not tmux.active(pane), "detach must restore native terminal ownership")
binding("y", primary).callback(window, pane)
assert(performed.args == "\x1b[115;9u", "existing native shells require the original widget sequence")
binding("Enter", primary .. "|SHIFT").callback(window, pane)
assert(performed.args == "\x01\x1b[13;2u\x02", "native ZLE needs one cursor-left byte")
print("tmux input routing: " .. (is_mac and "mac" or "linux") .. " passed")

dofile("shared/wezterm/tests/attach-remote.lua")
