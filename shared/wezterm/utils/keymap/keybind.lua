---@param action Action|RevAction
---@param mods string|string[]
---@param binds string|string[]
---@return BindKey
local function keybind(action, mods, binds)
  local rev = type(action) == "table" and rawget(action, "rev")
  return {
    key = binds,
    mods = mods,
    action = rev and action.action or action,
    rev = rev or nil,
  }
end

return keybind