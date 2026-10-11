---@class RevAction
---@field action Action
---@field rev Action bound with each mod's MOD.REV

---@param action Action|RevAction
---@param mods string|string[] Alternative modifier combinations, e.g. { "CMD", "CTRL|SHIFT" }.
---@param binds string|string[] Keys to bind with each modifier combination.
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