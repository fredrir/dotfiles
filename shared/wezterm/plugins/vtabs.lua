local wezterm = require "wezterm"
local profiles = require "ui.colors.profiles"
local adopt_pane = require "utils.mux.adopt_pane"

local plugin_root = wezterm.home_dir .. "/projects/wez-plugins/vertical-tabs"
local package_path = plugin_root .. "/plugin/init.lua"

package.path = plugin_root .. "/plugin/?.lua;" .. plugin_root .. "/plugin/?/init.lua;" .. package.path
local vtabs = dofile(package_path) --[[@as TabsPlugin]]

local M = { plugin = vtabs, action = vtabs.action }

function M.apply_to_config(config)
  vtabs.apply_to_config(config, { settings = {}, hooks = { adoptable = adopt_pane.peer_domain } })
end

return M