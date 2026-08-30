local max_fps = require "ui.performance"
local gpu_adapters = require "utils.gpu-adapter"

---@type Config
local settings = {
  max_fps = max_fps,
  webgpu_preferred_adapter = gpu_adapters:pick_best(),
}

local M = {}

function M.apply_to_config(config)
  for key, value in pairs(settings) do
    config[key] = value
  end
end

return M
