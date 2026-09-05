return {
  "mrjones2014/smart-splits.nvim",
  commit = "0c7abbf2f4e64e56519cb036f01e62e83e2cdcf1",
  -- The pane-local @pane-is-vim marker must exist before the first navigation
  -- key. Upstream clears it on exit and suspend, and restores it on resume.
  lazy = false,
  init = function()
    if vim.env.TMUX and vim.env.TMUX ~= "" then
      vim.g.smart_splits_multiplexer_integration = "tmux"
    end
  end,
  opts = {
    at_edge = "stop",
    multiplexer_integration = vim.env.TMUX and vim.env.TMUX ~= "" and "tmux" or nil,
    disable_multiplexer_nav_when_zoomed = true,
    ignored_filetypes = { "neo-tree" },
  },
  config = function(_, opts)
    local splits = require "smart-splits"
    splits.setup(opts)
    for key, direction in pairs { h = "left", j = "down", k = "up", l = "right" } do
      vim.keymap.set({ "n", "t" }, "<C-" .. key .. ">", splits["move_cursor_" .. direction], {
        desc = "Move to " .. direction .. " split or tmux pane",
      })
    end
  end,
}
