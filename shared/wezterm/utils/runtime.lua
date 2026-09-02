---@param pane Pane
---@return boolean
local function pane_has_running_process(pane)
  local prog = pane:get_user_vars().WEZTERM_PROG
  return prog ~= nil and prog ~= ""
end

---@param tab MuxTab
---@return boolean
local function tab_has_running_process(tab)
  for _, info in ipairs(tab:panes_with_info()) do
    if pane_has_running_process(info.pane) then
      return true
    end
  end

  return false
end

return {
  pane_has_running_process = pane_has_running_process,
  tab_has_running_process = tab_has_running_process,
}
