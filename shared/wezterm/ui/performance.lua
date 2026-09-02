local platform = require "utils.platform"

---@type { max_fps: number?, front_end: ("OpenGL"|"Software"|"WebGpu")? }
return {
  max_fps = platform.is_mac and 120 or nil,
  front_end = platform.is_mac and "WebGpu" or nil,
}
