local wezterm = require "wezterm"
local profiles = require "ui.colors.profiles"

local plugin_root = wezterm.home_dir .. "/projects/wez-plugins/vertical-tabs"
local package_path = plugin_root .. "/plugin/init.lua"

package.path = plugin_root .. "/plugin/?.lua;" .. plugin_root .. "/plugin/?/init.lua;" .. package.path
local vtabs = dofile(package_path)

local M = { plugin = vtabs }

function M.apply_to_config(config)
  vtabs.apply_to_config(config, {
    settings = {
      background = profiles.active.colors.accent,
    },
  })
end

return M
