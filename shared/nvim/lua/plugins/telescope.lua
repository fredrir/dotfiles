---@type LazySpec
return {
  {
    "nvim-telescope/telescope.nvim",
    event = "VimEnter",
    cmd = "Telescope",
    dependencies = {
      "nvim-lua/plenary.nvim",
      { "nvim-telescope/telescope-fzf-native.nvim", build = "make", cond = vim.fn.executable "make" == 1 },
      "nvim-telescope/telescope-ui-select.nvim",
      "nvim-tree/nvim-web-devicons",
    },
    config = function()
      local telescope = require "telescope"
      telescope.setup {
        extensions = { ["ui-select"] = { require("telescope.themes").get_dropdown {} } },
      }
      pcall(telescope.load_extension, "fzf")
      pcall(telescope.load_extension, "ui-select")
    end,
  },
}
