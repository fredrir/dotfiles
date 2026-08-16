local design = require 'wez.design'
local wezterm = require 'wezterm'

local M = {}

local LEFT_CAP = '\u{e0b6}'
local RIGHT_CAP = '\u{e0b4}'

-- Query-only handle; wez.plugins.sync owns apply_to_config.
local sync_ok, sync = pcall(wezterm.plugin.require, 'https://github.com/fredrir/sync-panes.wez')

local refresh_at = {}

local function dmux_enabled()
  return os.getenv 'DMUX_WEZ_FIRST' == '1'
end

local function dmux_chips(window, pane)
  local palette = design.palette
  local marker, marker_err = require('wez.dmux_bridge.context').from_pane(pane)
  if not marker then
    return { { text = 'DMUX INVALID', fg = palette.crust, bg = palette.red } }, marker_err
  end

  local controller = require 'wez.dmux_bridge.controller'
  local cache, cache_err, cache_state = controller.cached_context(pane)
  if not cache then
    local now = os.time()
    if (refresh_at[marker.gui_pane_id] or 0) < now then
      refresh_at[marker.gui_pane_id] = now
      controller.background_refresh(pane)
    end
    local invalid = cache_state == 'invalid_marker'
      or cache_state == 'invalid_cache'
      or cache_state == 'invalid_context'
    return {
      {
        text = invalid and 'DMUX INVALID' or 'DMUX VERIFYING',
        fg = palette.crust,
        bg = invalid and palette.red or palette.yellow,
      },
    },
      cache_err
  end
  local display = cache.display
  local chips = {
    { text = display.logical_ref, fg = palette.lavender, bg = palette.mantle },
    { text = display.space_name, fg = palette.mauve, bg = palette.mantle },
    { text = display.owner_label .. ' (' .. display.owner_alias .. ')', fg = palette.peach, bg = palette.mantle },
    { text = display.backend, fg = palette.teal, bg = palette.mantle },
    { text = display.route, fg = palette.blue, bg = palette.mantle },
  }
  local ok, tab = pcall(function()
    return window:active_tab()
  end)
  if ok and tab then
    local summary = require('wez.dmux_bridge.context').tab_summary(tab)
    if summary.mixed then
      table.insert(chips, { text = 'MIXED', fg = palette.crust, bg = palette.red })
    end
    if summary.invalid > 0 then
      table.insert(chips, { text = 'UNSTAMPED', fg = palette.crust, bg = palette.red })
    end
  end
  return chips
end

-- Chips echo the tab language: quiet ones are colored text on mantle, loud
-- ones (modes that change what typing does) get a colored background. The
-- host chip is always present so macie/archie is peripheral knowledge.
local function chips_for(window, pane)
  local palette = design.palette
  local chips = {}

  if dmux_enabled() then
    for _, chip in ipairs(dmux_chips(window, pane)) do
      table.insert(chips, chip)
    end
  else
    local domain = pane and pane:get_domain_name() or nil
    if domain and domain ~= 'local' then
      table.insert(chips, { text = domain, fg = palette.peach, bg = palette.mantle })
    end

    local workspace = window:active_workspace()
    if workspace and workspace ~= 'default' then
      table.insert(chips, { text = workspace, fg = palette.mauve, bg = palette.mantle })
    end
  end

  if sync_ok and sync.is_synced(window) then
    table.insert(chips, { text = 'SYNC', fg = palette.crust, bg = palette.red })
  end

  if window:leader_is_active() then
    table.insert(chips, { text = 'LEADER', fg = palette.crust, bg = palette.lavender })
  end

  if not dmux_enabled() then
    table.insert(chips, { text = design.host, fg = palette.crust, bg = design.accent })
  end
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
