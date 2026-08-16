local wezterm = require 'wezterm'
local act = wezterm.action

local M = {}

local DOMAIN = 'unix'

-- dmux wez-first flag (plan §15.1, P5). The chosen mechanism is the
-- DMUX_WEZ_FIRST=1 environment variable, checked at config evaluation: it is
-- per-process, so a scratch/test GUI can opt in while the user's live GUI --
-- which was launched without the variable -- provably re-evaluates to the
-- unchanged legacy config on every reload. (A flag file was rejected for P5
-- because the live GUI would see it on its next reload.)
--
-- With the flag on, the GUI becomes an attach-only client of the
-- service-owned managed mux: it declares the `dmux` unix client domain at
-- the exact socket published by the service's runtime descriptor and starts
-- with `connect dmux` instead of spawning a local mux window. The descriptor
-- is authoritative; if it cannot be read, the domain is NOT declared (a
-- guessed socket path could dial the wrong server) and startup stays on the
-- default path with a logged error.
local function dmux_managed_descriptor()
  -- Mirror the service wrapper's runtime-dir resolution (§10.1):
  -- macOS <DARWIN_USER_TEMP_DIR>/dmux (TMPDIR is that confstr value for GUI
  -- processes), Linux $XDG_RUNTIME_DIR/dmux. DMUX_RUNTIME_DIR wins when a
  -- launcher provides it explicitly.
  local runtime = os.getenv 'DMUX_RUNTIME_DIR'
  if not runtime then
    local base
    if wezterm.target_triple:find 'darwin' then
      base = os.getenv 'TMPDIR'
    else
      base = os.getenv 'XDG_RUNTIME_DIR'
    end
    if not base then
      return nil, 'no DMUX_RUNTIME_DIR/TMPDIR/XDG_RUNTIME_DIR in environment'
    end
    runtime = base:gsub('/+$', '') .. '/dmux'
  end
  local path = runtime .. '/wez-dmux.json'
  local f = io.open(path, 'r')
  if not f then
    return nil, 'descriptor not readable: ' .. path
  end
  local body = f:read '*a'
  f:close()
  local ok, descriptor = pcall(wezterm.json_parse, body)
  if not ok or type(descriptor) ~= 'table' or type(descriptor.socket) ~= 'string' then
    return nil, 'descriptor unparsable: ' .. path
  end
  return descriptor
end

function M.apply(config)
  config.unix_domains = { {
    name = DOMAIN,
  } }

  if os.getenv 'DMUX_WEZ_FIRST' == '1' then
    local descriptor, why = dmux_managed_descriptor()
    if descriptor then
      table.insert(config.unix_domains, {
        name = 'dmux',
        socket_path = descriptor.socket,
        -- GUI-side defense in depth; the service manager is the only
        -- starter, and every dmux CLI call carries --no-auto-start.
        no_serve_automatically = true,
      })
      config.default_gui_startup_args = { 'connect', 'dmux' }
    else
      wezterm.log_error('dmux wez-first: attach-only startup disabled: ' .. tostring(why))
    end
  end

  local keys = config.keys or {}
  table.insert(keys, {
    key = 'u',
    mods = 'LEADER',
    action = act.SpawnCommandInNewTab {
      domain = {
        DomainName = DOMAIN,
      },
    },
  })
  -- Detaching is LEADER+d (wez.remote), which works for any domain.
  table.insert(keys, {
    key = 'A',
    mods = 'LEADER',
    action = act.AttachDomain(DOMAIN),
  })
  config.keys = keys
end

return M
