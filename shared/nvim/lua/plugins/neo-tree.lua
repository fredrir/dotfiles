return {
  "nvim-neo-tree/neo-tree.nvim",
  version = "*",
  dependencies = {
    "nvim-lua/plenary.nvim",
    "nvim-tree/nvim-web-devicons",
    "MunifTanjim/nui.nvim",
  },
  lazy = false,
  ---@module 'neo-tree'
  ---@type neotree.Config
  opts = {
    -- Also clean placeholders from session files created outside `:restart`.
    auto_clean_after_session_restore = true,
    filesystem = {
      filtered_items = {
        visible = true,
        hide_dotfiles = false,
      },
      window = {
        mappings = require("core.keymaps").neo_tree_window,
      },
    },
  },
}
