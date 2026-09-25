local theme = require "theme"

---@type LazySpec
return {
  {
    "catppuccin/nvim",
    name = "catppuccin",
    priority = 1000,
    opts = theme.options,
    config = function(_, opts)
      require("catppuccin").setup(opts)
      vim.cmd.colorscheme(theme.colorscheme)
    end,
  },
}
