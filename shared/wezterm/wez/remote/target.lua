local platform = require 'wez.platform'

-- The single place that knows where the tmux server lives. On macie it is on
-- archie, and `ssa` owns how to reach it; on archie it is the local server.
-- Nothing else in the config builds an ssh or tmux command line.
local M = {}

function M.is_remote()
  return platform.is_mac
end

function M.label()
  return M.is_remote() and 'archie' or 'local'
end

-- tmux allows spaces, dots and shell metacharacters in session names. The
-- remote path goes through a shell, so anything outside this set is refused
-- rather than escaped: `ssa` applies the same rule and there is no value in
-- being more permissive than the thing we are calling.
function M.is_valid_session(session)
  return type(session) == 'string' and session ~= '' and session:match '^[%w._-]+$' ~= nil
end

function M.list_command()
  if M.is_remote() then
    return { 'zsh', '-ic', 'ssa ls' }
  end
  return { 'tmux', 'list-sessions', '-F', '#{session_name}' }
end

-- Returns nil for a name that must not reach a shell.
function M.attach_command(session)
  if not M.is_valid_session(session) then
    return nil
  end
  if M.is_remote() then
    -- Passed as a positional parameter, never interpolated into the script,
    -- so the name cannot terminate the command and start another.
    return { 'zsh', '-ic', 'ssa --cc "$1"', 'zsh', session }
  end
  return { 'tmux', '-CC', 'new-session', '-A', '-s', session }
end

return M
