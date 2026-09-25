local M = {}

local map = vim.keymap.set
local autocmd = vim.api.nvim_create_autocmd
local command = vim.api.nvim_create_user_command

local session = require "utils.session"
local search = require "utils.search"

for name, action in pairs { W = "w", Q = "q", WQ = "wq", Wq = "wq" } do
  command(name, action, {})
end

command("Format", function()
  ---@type conform.FormatOpts
  local opts = { async = true, lsp_format = "fallback" }
  local _ = require("conform").format(opts)
end, { desc = "Format the buffer, falling back to LSP formatting" })

command("NeovimRestart", session.restart, { desc = "Restart Neovim, restoring the session" })
command("NeovimClose", session.close, { desc = "Write all buffers and quit" })
command("NeovimSync", session.sync, { desc = "Sync Lazy & Restart Neovim" })

command("SearchFiles", search.files, { desc = "Search files" })
command("SearchGrep", search.grep, { desc = "Search by grep" })
command("SearchGrepOpen", search.open_files, { desc = "Search open files by grep" })
command("SearchBuffer", search.buffer, { desc = "Fuzzily search the current buffer" })
command("SearchConfig", search.neovim, { desc = "Search the Neovim configuration" })
command("TerminalSearch", search.terminal, { desc = "Search files from the terminal" })

local lazygit
command("Lazygit", function()
  lazygit = lazygit
      or require("toggleterm.terminal").Terminal:new {
        cmd = "lazygit",
        hidden = true,
        direction = "float",
        float_opts = {
          border = "curved",
          width = function()
            return math.floor(vim.o.columns * 0.95)
          end,
          height = function()
            return math.floor(vim.o.lines * 0.9)
          end,
        },
      }
  lazygit:toggle()
end, { desc = "Toggle lazygit in a floating terminal" })

command("HarpoonAdd", function()
  require("harpoon"):list():add()
end, { desc = "Harpoon: add file" })

command("HarpoonMenu", function()
  local harpoon = require "harpoon"
  harpoon.ui:toggle_quick_menu(harpoon:list())
end, { desc = "Harpoon: quick menu" })

command("HarpoonSelect", function(opts)
  require("harpoon"):list():select(tonumber(opts.args))
end, { nargs = 1, desc = "Harpoon: select file by index" })

autocmd({ "FocusGained", "BufEnter", "CursorHold" }, {
  group = vim.api.nvim_create_augroup("checktime", { clear = true }),
  desc = "Reload files changed outside Neovim",
  command = "checktime",
})

autocmd("TextYankPost", {
  group = vim.api.nvim_create_augroup("highlight-yank", { clear = true }),
  desc = "Highlight yanks",
  callback = vim.hl.on_yank,
})

---@param bufnr integer
function M.gitsigns(bufnr)
  local gitsigns = require "gitsigns"

  local function nav(direction)
    if vim.wo.diff then
      vim.cmd.normal { direction == "next" and "]c" or "[c", bang = true }
    else
      gitsigns.nav_hunk(direction)
    end
  end

  map("n", "]c", function()
    nav "next"
  end, { buffer = bufnr, desc = "Jump to next git [c]hange" })
  map("n", "[c", function()
    nav "prev"
  end, { buffer = bufnr, desc = "Jump to previous git [c]hange" })
  map("v", "<leader>hs", function()
    gitsigns.stage_hunk { vim.fn.line ".", vim.fn.line "v" }
  end, { buffer = bufnr, desc = "git [s]tage hunk" })
  map("v", "<leader>hr", function()
    gitsigns.reset_hunk { vim.fn.line ".", vim.fn.line "v" }
  end, { buffer = bufnr, desc = "git [r]eset hunk" })
  map("n", "<leader>gs", "<cmd>Gitsigns stage_hunk<CR>", { buffer = bufnr, desc = "git [s]tage hunk" })
  map("n", "<leader>gr", "<cmd>Gitsigns reset_hunk<CR>", { buffer = bufnr, desc = "git [r]eset hunk" })
  map("n", "<leader>gS", "<cmd>Gitsigns stage_buffer<CR>", { buffer = bufnr, desc = "git [S]tage buffer" })
  map("n", "<leader>gu", "<cmd>Gitsigns undo_stage_hunk<CR>", { buffer = bufnr, desc = "git [u]ndo stage hunk" })
  map("n", "<leader>gR", "<cmd>Gitsigns reset_buffer<CR>", { buffer = bufnr, desc = "git [R]eset buffer" })
  map("n", "<leader>gp", "<cmd>Gitsigns preview_hunk<CR>", { buffer = bufnr, desc = "git [p]review hunk" })
  map("n", "<leader>gb", "<cmd>Gitsigns blame_line<CR>", { buffer = bufnr, desc = "git [b]lame line" })
  map("n", "<leader>gd", "<cmd>Gitsigns diffthis<CR>", { buffer = bufnr, desc = "git [d]iff against index" })
  map("n", "<leader>gD", "<cmd>Gitsigns diffthis @<CR>", { buffer = bufnr, desc = "git [D]iff against last commit" })
  map("n", "<leader>tb", "<cmd>Gitsigns toggle_current_line_blame<CR>", {
    buffer = bufnr,
    desc = "[T]oggle git show [b]lame line",
  })
  map("n", "<leader>tD", "<cmd>Gitsigns preview_hunk_inline<CR>", {
    buffer = bufnr,
    desc = "[T]oggle git show [D]eleted",
  })
end

local function highlight_references(bufnr)
  local group = vim.api.nvim_create_augroup("lsp-highlight", { clear = false })
  vim.api.nvim_clear_autocmds { group = group, buffer = bufnr }
  autocmd({ "CursorHold", "CursorHoldI" }, {
    buffer = bufnr,
    group = group,
    callback = vim.lsp.buf.document_highlight,
  })
  autocmd({ "CursorMoved", "CursorMovedI" }, {
    buffer = bufnr,
    group = group,
    callback = vim.lsp.buf.clear_references,
  })
end

autocmd("LspAttach", {
  group = vim.api.nvim_create_augroup("lsp-attach", { clear = true }),
  callback = function(event)
    local bufnr = event.buf
    local client = vim.lsp.get_client_by_id(event.data.client_id)

    map("n", "grn", vim.lsp.buf.rename, { buffer = bufnr, desc = "LSP: [R]e[n]ame" })
    map("n", "<leader>rn", vim.lsp.buf.rename, { buffer = bufnr, desc = "LSP: [R]e[n]ame" })
    map({ "n", "x" }, "gra", vim.lsp.buf.code_action, { buffer = bufnr, desc = "LSP: [G]oto Code [A]ction" })
    map("n", "grD", vim.lsp.buf.declaration, { buffer = bufnr, desc = "LSP: [G]oto [D]eclaration" })
    map("n", "grr", "<cmd>Telescope lsp_references<CR>", { buffer = bufnr, desc = "[G]oto [R]eferences" })
    map("n", "gri", "<cmd>Telescope lsp_implementations<CR>", { buffer = bufnr, desc = "[G]oto [I]mplementation" })
    map("n", "grd", "<cmd>Telescope lsp_definitions<CR>", { buffer = bufnr, desc = "[G]oto [D]efinition" })
    map("n", "grt", "<cmd>Telescope lsp_type_definitions<CR>", { buffer = bufnr, desc = "[G]oto [T]ype Definition" })
    map("n", "gO", "<cmd>Telescope lsp_document_symbols<CR>", { buffer = bufnr, desc = "Open Document Symbols" })
    map("n", "gW", "<cmd>Telescope lsp_dynamic_workspace_symbols<CR>", {
      buffer = bufnr,
      desc = "Open Workspace Symbols",
    })

    if client and client:supports_method("textDocument/inlayHint", bufnr) then
      map("n", "<leader>th", function()
        vim.lsp.inlay_hint.enable(not vim.lsp.inlay_hint.is_enabled { bufnr = bufnr }, { bufnr = bufnr })
      end, { buffer = bufnr, desc = "LSP: [T]oggle Inlay [H]ints" })
    end

    if client and client:supports_method("textDocument/documentHighlight", bufnr) then
      highlight_references(bufnr)
    end
  end,
})

autocmd("LspDetach", {
  group = vim.api.nvim_create_augroup("lsp-detach", { clear = true }),
  callback = function(event)
    vim.lsp.buf.clear_references()
    local group = vim.api.nvim_create_augroup("lsp-highlight", { clear = false })
    vim.api.nvim_clear_autocmds { group = group, buffer = event.buf }
  end,
})

return M
