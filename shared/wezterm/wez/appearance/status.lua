local design = require 'wez.design'
local wezterm = require 'wezterm'

local M = {}

local LEFT_CAP = '\u{e0b6}'
local RIGHT_CAP = '\u{e0b4}'

-- Query-only handle; wez.plugins.sync owns apply_to_config.
local sync_ok, sync = pcall(wezterm.plugin.require, 'https://github.com/fredrir/sync-panes.wez')

-- Chips echo the tab language: quiet ones are colored text on mantle, loud
-- ones (modes that change what typing does) get a colored background. The
-- host chip is always present so macie/archie is peripheral knowledge.
local function chips_for(window, pane)
  local palette = design.palette
  local chips = {}

  local domain = pane and pane:get_domain_name() or nil
  if domain and domain ~= 'local' then
    table.insert(chips, { text = domain, fg = palette.peach, bg = palette.mantle })
  end

  local workspace = window:active_workspace()
  if workspace and workspace ~= 'default' then
    table.insert(chips, { text = workspace, fg = palette.mauve, bg = palette.mantle })
  end

  if sync_ok and sync.is_synced(window) then
    table.insert(chips, { text = 'SYNC', fg = palette.crust, bg = palette.red })
  end

  if window:leader_is_active() then
    table.insert(chips, { text = 'LEADER', fg = palette.crust, bg = palette.lavender })
  end

  table.insert(chips, { text = design.host, fg = palette.crust, bg = design.accent })
  return chips
end

function M.setup()
  wezterm.on('update-status', function(window, pane)
    local elements = {}
    for index, chip in ipairs(chips_for(window, pane)) do
      if index > 1 then
        table.insert(elements, { Text = ' ' })
      end
      table.insert(elements, { Background = { Color = design.tabs.background } })
      table.insert(elements, { Foreground = { Color = chip.bg } })
      table.insert(elements, { Text = LEFT_CAP })
      table.insert(elements, { Background = { Color = chip.bg } })
      table.insert(elements, { Foreground = { Color = chip.fg } })
      table.insert(elements, { Text = chip.text })
      table.insert(elements, { Background = { Color = design.tabs.background } })
      table.insert(elements, { Foreground = { Color = chip.bg } })
      table.insert(elements, { Text = RIGHT_CAP })
    end
    table.insert(elements, { Text = ' ' })

    window:set_right_status(wezterm.format(elements))
  end)
end

return M
