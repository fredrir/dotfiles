---@class BindKey
---@field key string|string[]
---@field mods string|string[]
---@field action Action

---@param bindings BindKey[]
---@return Key[]
local function bind_keys(bindings)
  ---@type Key[]
  local keys = {}

  for _, binding in ipairs(bindings) do
    local binds = binding.key
    local mods = binding.mods

    if type(binds) == "string" then
      binds = { binds }
    end

    if type(mods) == "string" then
      mods = { mods }
    end

    for _, key in ipairs(binds) do
      for _, mod in ipairs(mods) do
        table.insert(keys, {
          key = key,
          mods = mod,
          action = binding.action,
        })
      end
    end
  end

  return keys
end

return bind_keys