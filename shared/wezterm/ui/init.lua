local max_fps = require "ui.performance"
local gpu_adapters = require "utils.gpu-adapter"

---@type Config
return {
  max_fps = max_fps,
  webgpu_preferred_adapter = gpu_adapters:pick_best(),
}
