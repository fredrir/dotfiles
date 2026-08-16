local wezterm = require 'wezterm'

local M = {}

local SOURCES = {
  'wez.integrations.clean_copy',
  'wez.integrations.ssh_hosts',
  'wez.integrations.command_palette',
}

local function each(fn)
  local sources = SOURCES
  if os.getenv 'DMUX_WEZ_FIRST' == '1' then
    -- ssh_hosts populates spawn-capable launch-menu entries and the custom
    -- command palette spawns tabs. clean-copy is presentation-neutral.
    sources = { 'wez.integrations.clean_copy' }
  end
  for _, name in ipairs(sources) do
    -- Per integration, so one failure costs only its own feature.
    local ok, err = pcall(function()
      fn(require(name))
    end)
    if not ok then
      wezterm.log_error('wezterm integrations: ' .. name .. ' failed: ' .. tostring(err))
    end
  end
end

function M.apply(config)
  each(function(mod)
    if mod.apply then
      mod.apply(config)
    end
  end)
end

function M.setup()
  each(function(mod)
    if mod.setup then
      mod.setup()
    end
  end)
end

return M
