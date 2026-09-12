local runtime = require "utils.editor"
local actions = require "utils.actions"
local languages = require "languages"
local theme = require "ui.theme"

---@param editor EditorConfig
---@param ui UiConfig
---@param keys KeymapConfig
---@return LazySpec
return function(editor, ui, keys)
  local completion = editor.completion
  return {
    -- Appearance
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
    {
      "nvim-mini/mini.nvim",
      config = function()
        require("mini.ai").setup(editor.textobjects)
        require("mini.surround").setup {}
        local statusline = require "mini.statusline"
        statusline.setup { use_icons = ui.statusline.use_icons }
        ---@diagnostic disable-next-line: duplicate-set-field
        statusline.section_location = function()
          return ui.statusline.location
        end
      end,
    },
    { "OXY2DEV/markview.nvim", lazy = false },
    {
      "folke/todo-comments.nvim",
      event = "VimEnter",
      dependencies = { "nvim-lua/plenary.nvim" },
      opts = ui.todo,
    },
    {
      "folke/which-key.nvim",
      event = "VimEnter",
      opts = { delay = editor.key_hints.delay, icons = { mappings = ui.nerd_font }, spec = keys.groups },
    },

    -- Editing and languages
    { "NMAC427/guess-indent.nvim", opts = {} },
    { "windwp/nvim-ts-autotag", event = { "BufReadPre", "BufNewFile" }, opts = {} },
    {
      "saghen/blink.cmp",
      event = "VimEnter",
      version = "1.*",
      dependencies = {
        {
          "L3MON4D3/LuaSnip",
          version = "2.*",
          build = runtime.make_build "make install_jsregexp",
          dependencies = {
            {
              "rafamadriz/friendly-snippets",
              config = function()
                require("luasnip.loaders.from_vscode").lazy_load()
              end,
            },
          },
          opts = {},
        },
      },
      ---@type blink.cmp.Config
      opts = {
        keymap = keys.completion,
        appearance = ui.completion,
        completion = { documentation = completion.documentation, list = { selection = completion.selection } },
        sources = {
          default = completion.sources,
          per_filetype = completion.sources_by_filetype,
          min_keyword_length = actions.minimum_keyword_length,
        },
        snippets = completion.snippets,
        fuzzy = completion.fuzzy,
        signature = completion.signature,
      },
    },
    {
      "neovim/nvim-lspconfig",
      dependencies = {
        { "mason-org/mason.nvim", opts = editor.tools.mason },
        "mason-org/mason-lspconfig.nvim",
        "WhoIsSethDaniel/mason-tool-installer.nvim",
        { "j-hui/fidget.nvim", opts = {} },
      },
      config = function()
        require("languages.lsp").setup(keys.lsp)
      end,
    },
    {
      "stevearc/conform.nvim",
      event = "BufWritePre",
      cmd = "ConformInfo",
      config = function()
        require("languages.format").setup(editor.formatting)
      end,
    },
    {
      "mfussenegger/nvim-lint",
      event = { "BufReadPre", "BufNewFile" },
      config = function()
        require("languages.lint").setup(editor.lint.events)
      end,
    },
    {
      "nvim-treesitter/nvim-treesitter",
      lazy = false,
      branch = "main",
      build = ":TSUpdate",
      config = function()
        require("languages.syntax").setup(editor.syntax)
      end,
    },

    -- Files and search
    {
      "nvim-neo-tree/neo-tree.nvim",
      version = "*",
      lazy = false,
      dependencies = { "nvim-lua/plenary.nvim", "nvim-tree/nvim-web-devicons", "MunifTanjim/nui.nvim" },
      opts = {
        auto_clean_after_session_restore = editor.files.clean_session_placeholders,
        filesystem = {
          filtered_items = { visible = editor.files.show_hidden, hide_dotfiles = not editor.files.show_hidden },
          window = { mappings = keys.neo_tree },
        },
      },
    },
    {
      "stevearc/oil.nvim",
      cmd = "Oil",
      dependencies = { "nvim-tree/nvim-web-devicons" },
      opts = { view_options = { show_hidden = editor.files.show_hidden } },
    },
    {
      "nvim-telescope/telescope.nvim",
      event = "VimEnter",
      cmd = "Telescope",
      dependencies = {
        "nvim-lua/plenary.nvim",
        { "nvim-telescope/telescope-fzf-native.nvim", build = "make", cond = runtime.has_make },
        "nvim-telescope/telescope-ui-select.nvim",
        { "nvim-tree/nvim-web-devicons", enabled = ui.nerd_font },
      },
      config = function()
        local telescope = require "telescope"
        telescope.setup {
          extensions = { ["ui-select"] = { require("telescope.themes").get_dropdown(ui.search.select) } },
        }
        pcall(telescope.load_extension, "fzf")
        pcall(telescope.load_extension, "ui-select")
      end,
    },
    {
      "ThePrimeagen/harpoon",
      branch = "harpoon2",
      lazy = true,
      dependencies = { "nvim-lua/plenary.nvim" },
      config = function()
        require("harpoon"):setup()
      end,
    },

    -- Git, terminals and diagnostics
    { "sindrets/diffview.nvim" },
    { "lewis6991/gitsigns.nvim", opts = { signs = ui.git, on_attach = runtime.on_attach(keys.gitsigns) } },
    {
      "akinsho/toggleterm.nvim",
      version = "*",
      cmd = { "ToggleTerm", "TermExec" },
      opts = ui.terminal.toggleterm,
    },
    { "folke/trouble.nvim", dependencies = { "nvim-tree/nvim-web-devicons" }, cmd = "Trouble", opts = {} },
    {
      "mfussenegger/nvim-dap",
      lazy = true,
      dependencies = {
        "rcarriga/nvim-dap-ui",
        "nvim-neotest/nvim-nio",
        "mason-org/mason.nvim",
        "jay-babu/mason-nvim-dap.nvim",
        "leoluz/nvim-dap-go",
      },
      config = function()
        local dap, dapui = require "dap", require "dapui"
        require("mason-nvim-dap").setup {
          automatic_installation = editor.debug.automatic_installation,
          handlers = {},
          ensure_installed = languages.list "debuggers",
        }
        dapui.setup(ui.debug)
        if editor.debug.open_ui_on_start then
          dap.listeners.after.event_initialized.dapui_config = dapui.open
        end
        if editor.debug.close_ui_on_end then
          dap.listeners.before.event_terminated.dapui_config = dapui.close
          dap.listeners.before.event_exited.dapui_config = dapui.close
        end
        require("dap-go").setup { delve = { detached = not runtime.is_windows() } }
      end,
    },
  }
end
