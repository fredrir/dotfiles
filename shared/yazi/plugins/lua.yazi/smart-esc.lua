--- @since 26.9.1
--- @sync entry

local function escapable()
  local tab = cx.active
  local current = tab.current
  return tab.finder ~= nil
    or tab.mode.is_visual
    or current.files.filter ~= nil
    or #tab.selected > 0
    or current.cwd.spec.is_search
end

local function entry()
  ya.emit(escapable() and "escape" or "quit", {})
end

return { entry = entry }