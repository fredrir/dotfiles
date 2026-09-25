---@type LazySpec
return {
  {
    "nvim-neo-tree/neo-tree.nvim",
    version = "*",
    lazy = false,
    dependencies = { "nvim-lua/plenary.nvim", "nvim-tree/nvim-web-devicons", "MunifTanjim/nui.nvim" },
    opts = {
      auto_clean_after_session_restore = true,
      filesystem = {
        filtered_items = { visible = true, hide_dotfiles = false },
        window = {
          mappings = {
            ["'"] = function()
              vim.cmd "wincmd p"
            end,
          },
        },
      },
    },
  },
}
