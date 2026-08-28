local wezterm = require('wezterm')

---@class WeztermConfigBuilder
---@field options Config
local Config = {}
Config.__index = Config

---Initialize Config
---@return WeztermConfigBuilder
function Config:init()
   local config = setmetatable({ options = {} }, self)
   return config
end

---@param new_options table 
---@return WeztermConfigBuilder
function Config:append(new_options)
   for k, v in pairs(new_options) do
      if self.options[k] ~= nil then
         wezterm.log_warn(
            'Duplicate config option detected: ',
            { old = self.options[k], new = new_options[k] }
         )
         goto continue
      end
      self.options[k] = v
      ::continue::
   end
   return self
end

return Config