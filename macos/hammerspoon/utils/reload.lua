---@param files string[]
---@return nil
return function(files)
  local doReload = false
  ---@type string
  local file
  for _, file in pairs(files) do
    if file:sub(-4) == ".lua" then
      doReload = true
    end
  end
  if doReload then
    hs.reload()
  end
end