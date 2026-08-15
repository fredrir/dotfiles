local wezterm = require 'wezterm'
local act = wezterm.action
local target = require 'wez.remote.target'

local M = {}

-- Both list commands emit one bare session name per line, but the ssh path
-- can interleave banner noise (motd, warnings). Only names we would be
-- willing to attach to are accepted; everything else is dropped.
function M.parse(stdout)
  local names = {}
  local seen = {}
  for line in stdout:gmatch '[^\r\n]+' do
    local name = line:match '^(%S+)$'
    if name and target.is_valid_session(name) and not seen[name] then
      seen[name] = true
      table.insert(names, name)
    end
  end
  return names
end

function M.list()
  -- run_child_process raises when the program is missing rather than returning
  -- false, so a bare `if not ok` would never see tmux being absent.
  local spawned, ok, stdout, stderr = pcall(wezterm.run_child_process, target.list_command())
  if not spawned then
    wezterm.log_warn('tmux sessions unavailable: ' .. tostring(ok))
    return nil, tostring(ok)
  end
  if not ok then
    wezterm.log_info('no tmux sessions: ' .. tostring(stderr))
    return {}
  end
  return M.parse(stdout)
end

function M.attach(window, pane, session)
  local args = target.attach_command(session)
  if not args then
    window:toast_notification('wezterm', 'Invalid session name: ' .. tostring(session), nil, 4000)
    return
  end
  window:perform_action(act.SpawnCommandInNewTab { args = args }, pane)
end

local function prompt_for_new(window, pane)
  window:perform_action(
    act.PromptInputLine {
      description = 'New tmux session name',
      action = wezterm.action_callback(function(w, p, line)
        if line and #line > 0 then
          M.attach(w, p, line)
        end
      end),
    },
    pane
  )
end

-- Offers existing sessions plus a "new session" entry that prompts for a name.
function M.picker()
  return wezterm.action_callback(function(window, pane)
    local names, err = M.list()
    if not names then
      window:toast_notification('wezterm', 'tmux unavailable: ' .. err, nil, 4000)
      return
    end

    local choices = {}
    for _, name in ipairs(names) do
      table.insert(choices, { id = name, label = name })
    end
    table.insert(choices, { id = '', label = '+ new session' })

    window:perform_action(
      act.InputSelector {
        title = 'tmux sessions (' .. target.label() .. ')',
        choices = choices,
        fuzzy = true,
        action = wezterm.action_callback(function(inner_window, inner_pane, id)
          if id == nil then
            return
          end
          if id == '' then
            prompt_for_new(inner_window, inner_pane)
          else
            M.attach(inner_window, inner_pane, id)
          end
        end),
      },
      pane
    )
  end)
end

return M
