return {
  "ThePrimeagen/harpoon",
  branch = "harpoon2",
  lazy = true,
  dependencies = { "nvim-lua/plenary.nvim" },
  config = function()
    require("harpoon"):setup()
  end,
}
