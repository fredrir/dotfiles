local wezterm = require "wezterm"
local MOD = require "keymap.modifiers"
local platform = require "utils.platform"
local extend = require "utils.extend"
local act = wezterm.action

---@type KeySpec[]
local motion_keys = {
  { key = "LeftArrow", mods = MOD.UNIQUE, action = act.SendKey { key = "b", mods = "ALT" } },
  { key = "RightArrow", mods = MOD.UNIQUE, action = act.SendKey { key = "f", mods = "ALT" } },
  { key = "LeftArrow", mods = MOD.PRIMARY, action = act.SendKey { key = "a", mods = "CTRL" } },
  { key = "RightArrow", mods = MOD.PRIMARY, action = act.SendKey { key = "e", mods = "CTRL" } },
  {
    key = "Enter",
    mods = "SHIFT",
    action = act.SendString "\x16\n", -- Split the line at the cursor
  },
  {
    key = "Enter",
    mods = MOD.PRIMARY,
    action = act.Multiple {
      act.SendKey { key = "e", mods = "CTRL" },
      act.SendString "\x16\n",
    },
  },
  {
    key = "Enter",
    mods = MOD.PRIMARY .. "|SHIFT",
    action = act.Multiple {
      act.SendKey { key = "a", mods = "CTRL" },
      act.SendString "\x16\n",
      act.SendKey { key = "b", mods = "CTRL" },
    },
  },
}

if platform.is_mac then
  extend(motion_keys, {
    { key = "Backspace", mods = MOD.PRIMARY, action = act.SendKey { key = "u", mods = "CTRL" } },
    {
      key = "Backspace",
      mods = MOD.SUPER_REV,
      action = act.Multiple {
        act.SendKey { key = "a", mods = "CTRL" }, -- beginning
        act.SendKey { key = "k", mods = "CTRL" }, -- delete to end
      },
    },
  })
end

return motion_keys
