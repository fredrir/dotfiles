local wezterm = require 'wezterm'

local M = {}

-- Every plugin is pinned by fork under github.com/fredrir: wezterm clones a
-- plugin once and never auto-updates, so both machines resolve identical
-- code until the fork itself moves. sync stays last so its generated key
-- table can mirror every binding added before it.
local SOURCES = {
  'wez.plugins.attention',
  'wez.plugins.resurrect',
  'wez.plugins.workspace_picker',
  'wez.plugins.stack',
  'wez.plugins.sync',
}

local function each(fn)
  for _, name in ipairs(SOURCES) do
    -- Per plugin, so one failure costs only its own feature.
    local ok, err = pcall(function()
      fn(require(name))
    end)
    if not ok then
      wezterm.log_error('wezterm plugins: ' .. name .. ' failed: ' .. tostring(err))
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
