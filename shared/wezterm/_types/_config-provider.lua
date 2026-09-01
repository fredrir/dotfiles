---@class ConfigBuilder: Config
---@field set_strict_mode fun(self: ConfigBuilder, strict: boolean)

---@alias ConfigFragment Config
---@alias ConfigProvider ConfigFragment|ConfigProviderFunction|ConfigProviderModule|string
---@alias ConfigProviderFunction fun(config: Config): ConfigFragment?

---@class ConfigProviderModule
---@field apply_to_config fun(config: Config): ConfigFragment?
