local wezterm = require 'wezterm'
local act = wezterm.action
local platform = require 'wez.platform'

-- Native mux to the peer machine. Both ssh domains reach the same
-- wezterm-mux-server, so windows and panes are identical through either
-- path: when the cable dies mid-session, attaching the -ts domain resumes
-- exactly where the -usb session stopped.
local M = {}

local PEER = platform.pick {
  mac = {
    name = 'archie',
    usb_address = '10.77.77.2',
    ts_address = '100.126.231.24',
    wezterm_path = '/usr/bin/wezterm',
    -- Mirrors the probe in macos/ssh/config.d/05-archie-cabled-first.
    probe = { '/usr/bin/nc', '-4', '-z', '-G', '1', '-b', 'en3', '10.77.77.2', '22' },
  },
  linux = {
    name = 'macie',
    usb_address = '10.77.77.1',
    ts_address = '100.75.71.79',
    wezterm_path = '/opt/homebrew/bin/wezterm',
    -- Mirrors the probe in linux/arch/ssh/config.d/05-macie-cabled-first.
    probe = { '/usr/bin/nc', '-z', '-w', '1', '-s', '10.77.77.2', '10.77.77.1', '22' },
  },
}

local function domain_name(suffix)
  return PEER.name .. '-' .. suffix
end

function M.domains()
  if not PEER then
    return {}
  end
  local function domain(suffix, address)
    return {
      name = domain_name(suffix),
      remote_address = address,
      username = 'fredrir',
      multiplexing = 'WezTerm',
      remote_wezterm_path = PEER.wezterm_path,
      assume_shell = 'Posix',
    }
  end
  -- Explicit addresses rather than ~/.ssh/config aliases: wezterm's built-in
  -- ssh client parses only a subset of that config, and the cabled-first
  -- `Match exec` probe is not in the subset. Path selection happens in
  -- attach_action instead.
  return { domain('usb', PEER.usb_address), domain('ts', PEER.ts_address) }
end

function M.usb_reachable()
  if not PEER then
    return false
  end
  local ok, success = pcall(wezterm.run_child_process, PEER.probe)
  return ok and success
end

-- Attach the peer through the USB link when it answers, Tailscale otherwise.
-- Spawning a tab in the domain attaches it, which also brings along every
-- window already live on the remote mux server.
function M.attach_action()
  return wezterm.action_callback(function(window, pane)
    if not PEER then
      return
    end
    local name = domain_name(M.usb_reachable() and 'usb' or 'ts')
    window:perform_action(act.SpawnCommandInNewTab { domain = { DomainName = name } }, pane)
  end)
end

return M
