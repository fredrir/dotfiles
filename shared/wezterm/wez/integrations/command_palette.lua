local wezterm = require 'wezterm'
local act = wezterm.action
local platform = require 'wez.platform'

local M = {}

-- Commands are spawned into a tab rather than run in the background so a
-- failure is visible instead of only reaching the log.
local function entry(brief, args, icon)
  return {
    brief = brief,
    icon = icon,
    action = act.SpawnCommandInNewTab { args = args },
  }
end

local function commands()
  local list = {
    entry('Regenerate theme', { 'generate-theme' }, 'md_palette'),
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
  wezterm.on('augment-command-palette', function(_window, _pane)
    return commands()
  end)
end

return M
