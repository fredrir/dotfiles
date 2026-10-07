---@param action Action
---@param mods string|string[] Alternative modifier combinations, e.g. { "CMD", "CTRL|SHIFT" }.
---@param binds string|string[] Keys to bind with each modifier combination.
---@return BindKey
local function keybind(action, mods, binds)
  return {
    key = binds,
    mods = mods,
    action = action,
  }
end

return keybind
