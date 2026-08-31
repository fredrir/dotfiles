local platform = require("utils.platform")

---@type { max_fps: number? }
return {
	max_fps = platform.is_mac and 120 or nil,
}
