local shell_filetypes = { bash = true, sh = true, zsh = true }
local shell_sources = { "lsp", "path", "snippets", "buffer" }

---@type LazySpec
return {
  {
    "saghen/blink.cmp",
    event = "VimEnter",
    version = "1.*",
    ---@type blink.cmp.Config
    opts = {
      keymap = {
        preset = "default",
        ["<CR>"] = { "accept", "fallback" },
        ["<Tab>"] = {
          "select_next",
          "snippet_forward",
          function(cmp)
            if not shell_filetypes[vim.bo.filetype] then
              return
            end
            local before = vim.api.nvim_get_current_line():sub(1, vim.api.nvim_win_get_cursor(0)[2])
            if before:match "%S$" then
              return cmp.show { initial_selected_item_idx = 1 }
            end
          end,
          "fallback",
        },
        ["<S-Tab>"] = { "select_prev", "snippet_backward", "fallback" },
      },
      appearance = { nerd_font_variant = "mono" },
      snippets = { preset = "default" },
      completion = {
        documentation = { auto_show = false, auto_show_delay_ms = 250 },
        list = { selection = { preselect = false, auto_insert = false } },
      },
      sources = {
        default = { "lsp", "path", "snippets" },
        per_filetype = {
          sh = shell_sources,
          bash = shell_sources,
          zsh = shell_sources,
          markdown = { "lsp", "path", "buffer" },
        },
        min_keyword_length = function(ctx)
          return vim.bo[ctx.bufnr].filetype == "markdown" and 3 or 0
        end,
      },
      fuzzy = { implementation = "lua" },
      signature = { enabled = true },
    },
  },
}
