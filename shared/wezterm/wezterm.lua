local wezterm = require "wezterm" ---@type Wezterm
local append_conf = require "utils.append_conf"

local config = wezterm.config_builder()
---@cast config ConfigBuilder
config:set_strict_mode(true)

return append_conf(config)
