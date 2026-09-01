local profile = require "core.profile"

if profile.minimal then
  return {}
end

return {
  "stevearc/conform.nvim",
  event = { "BufWritePre" },
  cmd = { "ConformInfo" },
  opts = function()
    return require "languages.formatters"
  end,
}
