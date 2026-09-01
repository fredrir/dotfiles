local keymaps = require "core.keymaps"

return {
  "nvim-telescope/telescope.nvim",
  event = "VimEnter",
  cmd = { "Telescope", "TerminalSearch" },
  dependencies = {
    "nvim-lua/plenary.nvim",
    {
      "nvim-telescope/telescope-fzf-native.nvim",
      build = "make",
      cond = function()
        return vim.fn.executable "make" == 1
      end,
    },
    { "nvim-telescope/telescope-ui-select.nvim" },
    { "nvim-tree/nvim-web-devicons", enabled = vim.g.have_nerd_font },
  },
  config = function()
    require("telescope").setup {
      extensions = {
        ["ui-select"] = { require("telescope.themes").get_dropdown() },
      },
    }

    pcall(require("telescope").load_extension, "fzf")
    pcall(require("telescope").load_extension, "ui-select")

    local builtin = require "telescope.builtin"
    local actions = require "telescope.actions"
    local action_state = require "telescope.actions.state"
    local search_files
    local search_grep

    local function picker_options(opts, prompt_title)
      local terminal = opts and opts.terminal
      local options = vim.tbl_deep_extend("force", {
        prompt_title = prompt_title,
        attach_mappings = keymaps.telescope_picker {
          terminal = terminal,
          search_files = search_files,
          search_grep = search_grep,
          actions = actions,
          action_state = action_state,
        },
      }, opts or {})
      options.terminal = nil
      return options
    end

    search_files = function(opts)
      builtin.find_files(picker_options(opts, keymaps.telescope_prompt_titles.files))
    end

    search_grep = function(opts)
      builtin.live_grep(picker_options(opts, keymaps.telescope_prompt_titles.grep))
    end

    vim.api.nvim_create_user_command("TerminalSearch", function()
      vim.schedule(function()
        search_files { terminal = true }
      end)
    end, {})

    keymaps.telescope(builtin, search_files, search_grep)
  end,
}
