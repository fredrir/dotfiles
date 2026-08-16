local wezterm = require 'wezterm'
local act = wezterm.action
local platform = require 'wez.platform'

local M = {}

-- TODO Refactor, Redesign and Reimplement.

local function entry(brief, args, icon)
  return {
    brief = brief,
    icon = icon,
    action = act.SpawnCommandInNewTab {
      args = args,
    },
  }
end

local function commands()
  local list = {
    entry('Regenerate theme', { 'dotfile', 'theme', 'apply' }, 'md_palette'),
    entry('dotfile check', { 'dotfile', 'check' }, 'md_check_circle_outline'),
    entry('dotfile status', { 'dotfile', 'status' }, 'md_format_list_bulleted'),
    entry('dotfile link', { 'dotfile', 'link' }, 'md_link_variant'),
    entry('Capture transcript', { 'transcript', 'capture' }, 'md_notebook'),
  }

  -- cpa/acp push the clipboard to archie, so they only mean anything on macie.
  if platform.is_mac then
    table.insert(list, entry('Clipboard to archie', { 'cpa' }, 'md_content_copy'))
    table.insert(list, entry('Clipboard from archie', { 'acp' }, 'md_content_paste'))
  end

  return list
end

function M.setup()
  if os.getenv 'DMUX_WEZ_FIRST' == '1' then
    -- Every legacy entry spawns a native tab. Managed mode leaves them out;
    -- tools remain runnable inside a pane in the ordinary way.
    return
  end
  wezterm.on('augment-command-palette', function(_window, _pane)
    return commands()
  end)
end

return M
