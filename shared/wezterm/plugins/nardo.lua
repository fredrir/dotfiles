-- local wezterm = require "wezterm"
-- local MOD = require "keymap.modifiers"

-- local plugin_root = wezterm.home_dir .. "/projects/wez-plugins/nardo/plugin"
-- package.path = plugin_root .. "/?.lua;" .. plugin_root .. "/?/init.lua;" .. package.path

-- local nardo = dofile(plugin_root .. "/init.lua")

-- local M = {}

-- function M.apply_to_config(config)
--   nardo.apply_to_config(config, {
--     backend = {
--       path = function(domain, host)
--         if host == "archie" or domain:match "^archie" then
--           return "/home/fredrir/projects/wez-plugins/nardo/backend/target/release/wez-nardo"
--         end
--         return wezterm.home_dir .. "/projects/wez-plugins/nardo/backend/target/release/wez-nardo"
--       end,
--     },
--     sessions = { key = { key = "k", mods = MOD.PRIMARY } },
--   })
-- end

-- return M
