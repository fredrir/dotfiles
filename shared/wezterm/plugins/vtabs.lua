local wezterm = require "wezterm"

local plugin_root = wezterm.home_dir .. "/projects/wez-plugins/vertical-tabs/plugin"
package.path = plugin_root .. "/?.lua;" .. plugin_root .. "/?/init.lua;" .. package.path

local vtabs = dofile(plugin_root .. "/init.lua")

local M = { plugin = vtabs }

function M.apply_to_config(config)
  vtabs.apply_to_config(config, {
    dim_inactive_panes = true,
    backend = {
      path = function(domain, host)
        if host == "archie" or domain:match "^archie" then
          return "/home/fredrir/projects/wez-plugins/vertical-tabs/backend/target/release/wez-vtabs"
        end
        return wezterm.home_dir .. "/projects/wez-plugins/vertical-tabs/backend/target/release/wez-vtabs"
      end,
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
