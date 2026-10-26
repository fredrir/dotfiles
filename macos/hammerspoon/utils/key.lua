local M = {}

M.taps = {}

---@param mods string[]
---@param key string
---@param action fun()
---@return hs.eventtap
function M.createKeybind(mods, key, action)
  local tap = hs.eventtap.new({ hs.eventtap.event.types.keyDown }, function(event)
    if event:getFlags():containExactly(mods) and event:getKeyCode() == hs.keycodes.map[key] then
      action()
      return true
    end

    return false
  end)

  tap:start()
  table.insert(M.taps, tap)

  return tap
end

local appDirs = { "/Applications/", "/System/Applications/", "/System/Applications/Utilities/" }

---@param mods string[]
---@param key string
---@param application string
---@return hs.eventtap
function M.openApp(mods, key, application)
  return M.createKeybind(mods, key, function()
    for _, dir in ipairs(appDirs) do
      local path = dir .. application .. ".app"
      if hs.fs.attributes(path) then
        hs.application.launchOrFocus(path)
        return
      end
    end
  end)
end

return M