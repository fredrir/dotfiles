local wezterm = require "wezterm"
local is_remote = require "utils.remote"

local function is_found(str, pattern)
  return string.find(str, pattern) ~= nil
end

---@alias PlatformType 'windows' | 'linux' | 'mac'

---@class Platform
---@field os PlatformType
---@field is_win boolean
---@field is_linux boolean
---@field is_mac boolean
---@field is_remote fun(pane: Pane): boolean
---@return Platform
local function platform()
  local is_win = is_found(wezterm.target_triple, "windows")
  local is_linux = is_found(wezterm.target_triple, "linux")
  local is_mac = is_found(wezterm.target_triple, "apple")
  ---@type PlatformType
  local os

  if is_win then
    os = "windows"
  elseif is_linux then
    os = "linux"
  elseif is_mac then
    os = "mac"
  else
    error "Unknown platform"
  end

  return {
    os = os,
    is_win = is_win,
    is_linux = is_linux,
    is_mac = is_mac,
    is_remote = is_remote,
  }
end

local _platform = platform()

return _platform
