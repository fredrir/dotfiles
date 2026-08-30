local colors = require "ui.colors"
local fonts = require "ui.fonts"
local gpu_adapters = require "utils.gpu-adapter"
local performance = require "ui.performance"

---@type Config
local settings = {
  max_fps = performance.max_fps,
  webgpu_preferred_adapter = gpu_adapters:pick_best(),
}

local M = {}

function M.apply_to_config(config)
  for key, value in pairs(settings) do
    config[key] = value
  end
  colors.apply_to_config(config)
  fonts.apply_to_config(config)
end

return M
