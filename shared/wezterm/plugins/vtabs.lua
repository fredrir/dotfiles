local wezterm = require "wezterm"

package.path = wezterm.home_dir .. "/projects/wez-vertical-tabs/plugin/?.lua;" .. package.path

local vtabs = require "init"

local M = {}

function M.apply_to_config(config)
  vtabs.apply_to_config(config, {
    backend = {
      path = wezterm.home_dir .. "/projects/wez-vertical-tabs/backend/target/release/wez-vtabs",
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
