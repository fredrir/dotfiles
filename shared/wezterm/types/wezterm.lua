---@meta

---@class WeztermConfig
local Config = {}

---@param enabled boolean
function Config:set_strict_mode(enabled) end

local wezterm = {}

---@return WeztermConfig
function wezterm.config_builder() end

---@return string
function wezterm.hostname() end

return wezterm
