local platform = require 'wez.platform'

-- The single place that knows where the tmux server lives: on archie, for
-- both machines. From macie it is reached over ssh, and the `archie` alias
-- carries the cabled-first/Tailscale routing from ssh config. Nothing else
-- in the config builds an ssh or tmux command line.
local M = {}

function M.is_remote()
  return platform.is_mac
end

function M.label()
  return M.is_remote() and 'archie' or 'local'
end

-- tmux allows spaces, dots and shell metacharacters in session names. Names
-- outside this set are refused rather than escaped; `ssa` applies the same
-- rule, and there is no value in being more permissive than it.
function M.is_valid_session(session)
  return type(session) == 'string' and session:match '^[%w_-]+$' ~= nil
end

function M.list_command()
  if M.is_remote() then
    return { 'ssh', '-o', 'BatchMode=yes', 'archie', 'tmux', 'list-sessions', '-F', '#{session_name}' }
  end
  return { 'tmux', 'list-sessions', '-F', '#{session_name}' }
end

-- Returns nil for a name that must not reach a command line.
function M.attach_command(session)
  if not M.is_valid_session(session) then
    return nil
  end
  if M.is_remote() then
    return { 'ssh', '-t', 'archie', 'exec', 'tmux', 'new-session', '-A', '-s', session }
  end
  return { 'tmux', 'new-session', '-A', '-s', session }
end

return M
