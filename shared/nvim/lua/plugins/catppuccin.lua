return {
  "catppuccin/nvim",
  name = "catppuccin",
  priority = 1000,
  config = function()
    require("catppuccin").setup {
      flavour = "mocha",
      color_overrides = {
        all = {
          rosewater = "#ce8174",
          flamingo = "#bf616a",
          pink = "#b48ead",
          mauve = "#b48ead",
          red = "#bf616a",
          maroon = "#ce8174",
          peach = "#ebcb8b",
          yellow = "#ebcb8b",
          green = "#a3be8c",
          teal = "#88c0d0",
          sky = "#8cb7bc",
          sapphire = "#86acbf",
          blue = "#81a1c1",
          lavender = "#81a1c1",
          text = "#ffffff",
          subtext1 = "#e4e4e8",
          subtext0 = "#cacad1",
          overlay2 = "#b0b1ba",
          overlay1 = "#9798a4",
          overlay0 = "#91929f",
          surface2 = "#91929f",
          surface1 = "#313247",
          surface0 = "#313247",
          base = "#15152b",
          mantle = "#101020",
          crust = "#060419",
        },
      },
      no_italic = true,
      integrations = {
        gitsigns = true,
        treesitter = true,
        telescope = { enabled = true },
        which_key = true,
        mini = { enabled = true },
        indent_blankline = { enabled = true },
        native_lsp = {
          enabled = true,
          underlines = {
            errors = { "undercurl" },
            hints = { "undercurl" },
            warnings = { "undercurl" },
            information = { "undercurl" },
          },
        },
      },
    }
    vim.cmd.colorscheme "catppuccin"
  end,
}
