---@type LazySpec
return {
  {
    "folke/which-key.nvim",
    event = "VimEnter",
    opts = {
      delay = 0,
      icons = { mappings = true },
      spec = {
        { "<leader>s", group = "[S]earch", mode = { "n", "v" } },
        { "<leader>t", group = "[T]oggle" },
        { "<leader>g", group = "[G]it" },
        { "<leader>r", group = "[R]efactor / Restart" },
        { "<leader>l", group = "[L]azy" },
        { "<leader>w", group = "[W]indow Navigation" },
        { "gr", group = "LSP Actions", mode = { "n" } },
      },
    },
  },
}
