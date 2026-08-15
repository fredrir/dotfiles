local design = require 'wez.design'
local wezterm = require 'wezterm'

local M = {}

local LEFT_CAP = '\u{e0b6}'
local RIGHT_CAP = '\u{e0b4}'

-- Rendering-side handles only; wez.plugins owns the apply_to_config calls
-- (they must run after wez.keys, which replaces config.keys wholesale).
-- pcall: a failed clone degrades to "no indicators", not a broken tab bar.
local att_ok, attention = pcall(wezterm.plugin.require, 'https://github.com/fredrir/wezterm-attention')
local stack_ok, stack = pcall(wezterm.plugin.require, 'https://github.com/fredrir/stack.wez')

local ATT_PRIORITY = { thinking = 1, review = 2, stop = 3, notify = 4 }
local ATT_GLYPHS = {
  stop = '✓ ',
  notify = '! ',
  review = '◆ ',
  thinking = { '◌ ', '◔ ', '◑ ', '◕ ' },
}
local ATT_COLORS = {
  thinking = design.palette.mauve,
  stop = design.palette.green,
  notify = design.palette.red,
  review = design.palette.yellow,
}

-- The plugin's per-tab priority resolution is internal, so it is
-- re-implemented here; cache-only, zero IO per render.
local function tab_attention(tab)
  if not att_ok then
    return nil, nil
  end
  local best, best_priority, best_frame
  for _, pane in ipairs(tab.panes or { tab.active_pane }) do
    local kind, frame = attention.get_attention(pane.pane_id)
    if kind and (best_priority or 0) < (ATT_PRIORITY[kind] or 0) then
      best, best_priority, best_frame = kind, ATT_PRIORITY[kind], frame
    end
  end
  return best, best_frame
end

function M.apply(config)
  config.use_fancy_tab_bar = false
  config.tab_bar_at_bottom = false
  config.hide_tab_bar_if_only_one_tab = false
  config.show_new_tab_button_in_tab_bar = false
  config.tab_max_width = 26

  config.colors.tab_bar = {
    background = design.tabs.background,
    active_tab = {
      bg_color = design.tabs.active_bg,
      fg_color = design.tabs.active_fg,
    },
    inactive_tab = {
      bg_color = design.tabs.inactive_bg,
      fg_color = design.tabs.inactive_fg,
    },
    inactive_tab_hover = {
      bg_color = design.tabs.hover_bg,
      fg_color = design.tabs.hover_fg,
    },
  }
end

local function tab_title(tab)
  local title = tab.tab_title
  if title and #title > 0 then
    return title
  end
  return tab.active_pane.title
end

local MARKER_DIR = wezterm.home_dir .. '/.local/state/wezterm-attention'

function M.setup()
  if att_ok then
    -- Bridge terminal bells into the marker system, so a bell in a
    -- background tab shows the notify indicator and clears on focus.
    wezterm.on('bell', function(_window, pane)
      os.execute("mkdir -p '" .. MARKER_DIR .. "'")
      local file = io.open(MARKER_DIR .. '/' .. pane:pane_id(), 'w')
      if file then
        file:write('{"type":"notify","updated_at":' .. os.time() .. '}')
        file:close()
      end
    end)
  end

  wezterm.on('format-tab-title', function(tab, _tabs, _panes, _config, hover, max_width)
    -- The plugin's focus auto-clear only runs in its own renderer, so
    -- manual mode clears seen stop/notify markers here.
    if att_ok and tab.is_active and tab.active_pane then
      local kind = attention.get_attention(tab.active_pane.pane_id)
      if kind == 'stop' or kind == 'notify' then
        attention.remove_marker(tab.active_pane.pane_id)
      end
    end

    local kind, frame = tab_attention(tab)
    local indicator
    if kind then
      local glyph = ATT_GLYPHS[kind]
      indicator = type(glyph) == 'table' and glyph[(frame or 0) % #glyph + 1] or glyph
    end

    local badge
    if stack_ok then
      -- [i/n] while a stack (zoomed pane) hides the tab's other panes.
      local ok, info = pcall(stack.stack_info, tab.tab_id)
      if ok and info and info.index then
        badge = string.format('[%d/%d] ', info.index, info.count)
      end
    end

    local bg, fg
    if tab.is_active then
      bg, fg = design.tabs.active_bg, design.tabs.active_fg
    elseif hover then
      bg, fg = design.tabs.hover_bg, design.tabs.hover_fg
    else
      bg, fg = design.tabs.inactive_bg, design.tabs.inactive_fg
    end
    local index_fg = tab.is_active and design.tabs.index_active or design.tabs.index_inactive
    local index = string.format(' %d ', tab.tab_index + 1)

    local reserved = 2 + #index + (indicator and 2 or 0) + (badge and #badge or 0) + 1
    local title = wezterm.truncate_right(tab_title(tab), math.max(max_width - reserved, 1))

    local items = {
      { Background = { Color = design.tabs.background } },
      { Foreground = { Color = bg } },
      { Text = LEFT_CAP },
    }
    if indicator then
      table.insert(items, { Background = { Color = bg } })
      table.insert(items, { Foreground = { Color = ATT_COLORS[kind] } })
      table.insert(items, { Text = indicator })
    end
    table.insert(items, { Background = { Color = bg } })
    table.insert(items, { Foreground = { Color = index_fg } })
    table.insert(items, { Text = index })
    table.insert(items, { Foreground = { Color = fg } })
    table.insert(items, { Text = title .. ' ' })
    if badge then
      table.insert(items, { Foreground = { Color = design.tabs.badge } })
      table.insert(items, { Text = badge })
    end
    table.insert(items, { Background = { Color = design.tabs.background } })
    table.insert(items, { Foreground = { Color = bg } })
    table.insert(items, { Text = RIGHT_CAP })
    return items
  end)
end

return M
