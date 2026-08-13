local wezterm = require 'wezterm'
local act = wezterm.action

local M = {}

local function scratch_dir()
  return os.getenv 'XDG_RUNTIME_DIR' or os.getenv 'TMPDIR' or '/tmp'
end

-- clean-copy reads either the clipboard or stdin. The stdin form is used here
-- because the clipboard form needs a readback delay to avoid racing the copy.
-- run_child_process cannot supply stdin, so the selection goes through a file
-- in the user-private runtime dir, passed positionally so the text is never
-- part of the shell script.
local function clean_selection(window, pane)
  local text = window:get_selection_text_for_pane(pane)
  if not text or #text == 0 then
    return
  end

  local path = string.format('%s/wezterm-clean-copy.%s', scratch_dir(), pane:pane_id())
  local file, err = io.open(path, 'w')
  if not file then
    wezterm.log_error('clean-copy: cannot write ' .. path .. ': ' .. tostring(err))
    return
  end
  file:write(text)
  file:close()

  wezterm.background_child_process {
    'sh',
    '-c',
    'clean-copy --stdin < "$1"; rm -f "$1"',
    'sh',
    path,
  }
end

function M.apply(config)
  local keys = config.keys or {}
  -- CopyTo runs first so the raw selection is on the clipboard even when
  -- clean-copy is missing or fails; clean-copy then replaces it in place.
  table.insert(keys, {
    key = 'c',
    mods = 'CTRL|SHIFT',
    action = act.Multiple {
      act.CopyTo 'Clipboard',
      wezterm.action_callback(clean_selection),
    },
  })
  config.keys = keys
end

return M
