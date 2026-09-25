---@type LazySpec
return {
  {
    "akinsho/toggleterm.nvim",
    version = "*",
    cmd = { "ToggleTerm", "TermExec" },
    opts = {
      shade_terminals = false,
      direction = "float",
      size = function(term)
        if term.direction == "horizontal" then
          return 15
        end
        return vim.o.columns * 0.4
      end,
      float_opts = {
        border = "curved",
        winblend = 0,
        width = function()
          return math.floor(vim.o.columns * 0.85)
        end,
        height = function()
          return math.floor(vim.o.lines * 0.8)
        end,
      },
      highlights = { FloatBorder = { link = "FloatBorder" } },
    },
  },
}
