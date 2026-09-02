local wezterm = require "wezterm"

local plugin_root = wezterm.home_dir .. "/projects/wez-plugins/old.vertical-tabs"

local package_path = plugin_root .. "/plugin/init.lua"

package.path = plugin_root .. package_path .. ";" .. package.path

local vtabs = dofile(package_path)

local M = { plugin = vtabs }

function M.apply_to_config(config)
  vtabs.apply_to_config(config, {
    dim_inactive_panes = true,
    backend = {
      path = plugin_root .. "/backend/target/release/wez-vtabs",
    },
    keys = {
      new_tab = false,
      close_tab = false,
      new_window = false,
      next_tab = false,
    },
  })
end

return M
