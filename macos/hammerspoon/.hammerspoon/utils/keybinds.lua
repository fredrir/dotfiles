local M = {}

M.taps = {}

---@param mod string
---@param key string
---@param action fun()
---@return hs.eventtap
function M.createKeybind(mod, key, action)
    local tap = hs.eventtap.new(
        { hs.eventtap.event.types.keyDown },
        function(event)
            if event:getFlags():containExactly({ mod })
                and event:getKeyCode() == hs.keycodes.map[key] then
                action()
                return true
            end

            return false
        end
    )

    tap:start()
    table.insert(M.taps, tap)

    return tap
end

return M
