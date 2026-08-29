local platform = require "utils.platform"

---@type number|nil
local max_fps

if platform.is_mac then
  max_fps = 120
else
  max_fps = nil
end

return max_fps
