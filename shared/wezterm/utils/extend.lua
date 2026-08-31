---@generic T
---@param dst T[]
---@param src T[]
---@return T[]
return function(dst, src)
	table.move(src, 1, #src, #dst + 1, dst)
	return dst
end
