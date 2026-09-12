local M = {}

---@class lib.Mapping<T>
---@field mode string|string[]
---@field lhs string
---@field rhs string|fun()
---@field opts T

---@generic T
---@param mode string|string[]
---@param lhs string
---@param rhs string|fun()
---@param opts T
---@return lib.Mapping<T>
function M.mapping(mode, lhs, rhs, opts)
  return { mode = mode, lhs = lhs, rhs = rhs, opts = opts }
end

---@param values string[]
---@return string[]
function M.unique(values)
  local result, seen = {}, {}
  for _, value in ipairs(values) do
    if not seen[value] then
      result[#result + 1], seen[value] = value, true
    end
  end
  return result
end

---@param values string[]
---@return table<string, boolean>
function M.set(values)
  local result = {}
  for _, value in ipairs(values) do
    result[value] = true
  end
  return result
end

return M
