local map = vim.keymap.set
local actions = require "utils.actions"
local session = require "utils.session"
local search = require "utils.search"
local format = require "languages.format"
local lsp = require "languages.lsp"
local buffer_map = require("lib").mapping

-- General --

map("n", "<Esc>", "<cmd>nohlsearch<CR>")

-- [N]eovim / Editor --

map("n", "<leader>nr", session.restart, { desc = "Neovim Restart" })
map("n", "<leader>nq", session.close, { desc = "Neovim Close" })
map("n", "<leader>ns", "<cmd>Lazy sync<CR>", { desc = "Neovim Sync" })
map("n", "<leader>rr", session.restart, { desc = "Restart Neovim" })
map("n", "<leader>nl", "<cmd>Lazy<CR>", { desc = "Lazy Open" })

-- Editing --

map("v", "J", ":m '>+1<CR>gv=gv", { desc = "Move selection down" })
map("v", "K", ":m '<-2<CR>gv=gv", { desc = "Move selection up" })
map("n", "<leader>p", '"_dP', { desc = "Replace line with yanked content" })

-- Buffers --

map("n", "<S-h>", "<cmd>bprevious<CR>", { desc = "Previous buffer" })
map("n", "<S-l>", "<cmd>bnext<CR>", { desc = "Next buffer" })
map("n", "<leader>x", "<cmd>bdelete<CR>", { desc = "Close buffer" })

-- Windows --

map("n", "<C-h>", "<C-w><C-h>", { desc = "Move focus to the left window" })
map("n", "<C-l>", "<C-w><C-l>", { desc = "Move focus to the right window" })
map("n", "<C-j>", "<C-w><C-j>", { desc = "Move focus to the lower window" })
map("n", "<C-k>", "<C-w><C-k>", { desc = "Move focus to the upper window" })
map("t", "<C-h>", "<C-\\><C-n><C-w>h", { desc = "Move to left window" })
map("t", "<C-j>", "<C-\\><C-n><C-w>j", { desc = "Move to lower window" })
map("t", "<C-k>", "<C-\\><C-n><C-w>k", { desc = "Move to upper window" })
map("t", "<C-l>", "<C-\\><C-n><C-w>l", { desc = "Move to right window" })

-- Neotree --

map("n", "<leader>e", "<cmd>Neotree toggle<CR>", { desc = "File [E]xplorer" })
map("n", "'", "<cmd>Neotree focus<CR>", { desc = "Focus NeoTree" })

-- Window Navigation --

map("n", "<leader>w<Left>", "<cmd>leftabove vnew<CR>")
map("n", "<leader>w<Right>", "<cmd>rightbelow vnew<CR>")
map("n", "<leader>w<Up>", "<cmd>leftabove new<CR>")
map("n", "<leader>w<Down>", "<cmd>rightbelow new<CR>")

map("n", "<leader>wq", "<cmd>close<CR>", { desc = "Close current window" })

map("n", "-", "<cmd>Oil<CR>", { desc = "Open parent directory" })

-- Diagnostics --

map("n", "<leader>q", vim.diagnostic.setloclist, { desc = "Open diagnostic [Q]uickfix list" })
map("n", "<leader>d", "<cmd>Trouble diagnostics toggle<CR>", { desc = "Diagnostics (Trouble)" })

-- Formatting --

map("", "<leader>f", format.buffer, { desc = "[F]ormat buffer" })

-- Search --

map("n", "<leader>sh", "<cmd>Telescope help_tags<CR>", { desc = "[S]earch [H]elp" })
map("n", "<leader>sk", "<cmd>Telescope keymaps<CR>", { desc = "[S]earch [K]eymaps" })
map("n", "<leader>sf", search.files, { desc = "[S]earch [F]iles" })
map("n", "<leader>ss", "<cmd>Telescope builtin<CR>", { desc = "[S]earch [S]elect Telescope" })
map({ "n", "v" }, "<leader>sw", "<cmd>Telescope grep_string<CR>", { desc = "[S]earch current [W]ord" })
map("n", "<leader>sg", search.grep, { desc = "[S]earch by [G]rep" })
map("n", "<leader>sd", "<cmd>Telescope diagnostics<CR>", { desc = "[S]earch [D]iagnostics" })
map("n", "<leader>sr", "<cmd>Telescope resume<CR>", { desc = "[S]earch [R]esume" })
map("n", "<leader>s.", "<cmd>Telescope oldfiles<CR>", { desc = '[S]earch Recent Files ("." for repeat)' })
map("n", "<leader>sc", "<cmd>Telescope commands<CR>", { desc = "[S]earch [C]ommands" })
map("n", "<leader><leader>", "<cmd>Telescope buffers<CR>", { desc = "[ ] Find existing buffers" })

map("n", "<leader>/", search.buffer, { desc = "[/] Fuzzily search in current buffer" })
map("n", "<leader>s/", search.open_files, { desc = "[S]earch [/] in Open Files" })
map("n", "<leader>sn", search.neovim, { desc = "[S]earch [N]eovim files" })

-- Git --

map("n", "<leader>g", actions.git.lazygit, { desc = "Lazygit" })

-- Harpoon --

map("n", "<leader>a", actions.harpoon.add, { desc = "Harpoon: [A]dd file" })
map("n", "<C-e>", actions.harpoon.menu, { desc = "Harpoon: Quick menu" })
map("n", "<leader>1", actions.harpoon.select(1), { desc = "Harpoon file 1" })
map("n", "<leader>2", actions.harpoon.select(2), { desc = "Harpoon file 2" })
map("n", "<leader>3", actions.harpoon.select(3), { desc = "Harpoon file 3" })
map("n", "<leader>4", actions.harpoon.select(4), { desc = "Harpoon file 4" })

-- Debugging --

map("n", "<F5>", actions.debug.continue, { desc = "Debug: Start/Continue" })
map("n", "<F1>", actions.debug.step_into, { desc = "Debug: Step Into" })
map("n", "<F2>", actions.debug.step_over, { desc = "Debug: Step Over" })
map("n", "<F3>", actions.debug.step_out, { desc = "Debug: Step Out" })
map("n", "<leader>b", actions.debug.toggle_breakpoint, { desc = "Debug: Toggle Breakpoint" })
map("n", "<leader>B", actions.debug.conditional_breakpoint, { desc = "Debug: Set Breakpoint" })
map("n", "<F7>", actions.debug.toggle_ui, { desc = "Debug: See last session result." })

-- Terminal --

map("n", "<C-\\>", "<cmd>ToggleTerm<CR>", { desc = "Toggle terminal" })
map("t", "<C-\\>", "<cmd>ToggleTerm<CR>", { desc = "Toggle terminal" })
map("t", "<Esc><Esc>", "<C-\\><C-n>", { desc = "Exit terminal mode" })

---@class PickerKey
---@field mode string|string[]
---@field lhs string
---@field desc string

---@class PickerKeys
---@field switch table<utils.SearchKind, PickerKey>
---@field close_terminal PickerKey[]
---@field terminal_opts vim.keymap.set.Opts

---@class KeymapConfig
local M = {
  ---@type BufferMapping[]
  gitsigns = {
    buffer_map("n", "]c", actions.git.next_hunk, { desc = "Jump to next git [c]hange" }),
    buffer_map("n", "[c", actions.git.previous_hunk, { desc = "Jump to previous git [c]hange" }),
    buffer_map("v", "<leader>hs", actions.git.stage_selection, { desc = "git [s]tage hunk" }),
    buffer_map("v", "<leader>hr", actions.git.reset_selection, { desc = "git [r]eset hunk" }),
    buffer_map("n", "<leader>gs", "<cmd>Gitsigns stage_hunk<CR>", { desc = "git [s]tage hunk" }),
    buffer_map("n", "<leader>gr", "<cmd>Gitsigns reset_hunk<CR>", { desc = "git [r]eset hunk" }),
    buffer_map("n", "<leader>gS", "<cmd>Gitsigns stage_buffer<CR>", { desc = "git [S]tage buffer" }),
    buffer_map("n", "<leader>gu", "<cmd>Gitsigns stage_hunk<CR>", { desc = "git [u]ndo stage hunk" }),
    buffer_map("n", "<leader>gR", "<cmd>Gitsigns reset_buffer<CR>", { desc = "git [R]eset buffer" }),
    buffer_map("n", "<leader>gp", "<cmd>Gitsigns preview_hunk<CR>", { desc = "git [p]review hunk" }),
    buffer_map("n", "<leader>gb", "<cmd>Gitsigns blame_line<CR>", { desc = "git [b]lame line" }),
    buffer_map("n", "<leader>gd", "<cmd>Gitsigns diffthis<CR>", { desc = "git [d]iff against index" }),
    buffer_map("n", "<leader>gD", actions.git.diff_last_commit, { desc = "git [D]iff against last commit" }),
    buffer_map(
      "n",
      "<leader>tb",
      "<cmd>Gitsigns toggle_current_line_blame<CR>",
      { desc = "[T]oggle git show [b]lame line" }
    ),
    buffer_map("n", "<leader>tD", "<cmd>Gitsigns preview_hunk_inline<CR>", { desc = "[T]oggle git show [D]eleted" }),
  },
  ---@type BufferMapping[]
  lsp = {
    buffer_map("n", "grn", vim.lsp.buf.rename, { desc = "LSP: [R]e[n]ame" }),
    buffer_map("n", "<leader>rn", vim.lsp.buf.rename, { desc = "LSP: [R]e[n]ame" }),
    buffer_map({ "n", "x" }, "gra", vim.lsp.buf.code_action, { desc = "LSP: [G]oto Code [A]ction" }),
    buffer_map("n", "grD", vim.lsp.buf.declaration, { desc = "LSP: [G]oto [D]eclaration" }),
    buffer_map("n", "grr", "<cmd>Telescope lsp_references<CR>", { desc = "[G]oto [R]eferences" }),
    buffer_map("n", "gri", "<cmd>Telescope lsp_implementations<CR>", { desc = "[G]oto [I]mplementation" }),
    buffer_map("n", "grd", "<cmd>Telescope lsp_definitions<CR>", { desc = "[G]oto [D]efinition" }),
    buffer_map("n", "gO", "<cmd>Telescope lsp_document_symbols<CR>", { desc = "Open Document Symbols" }),
    buffer_map("n", "gW", "<cmd>Telescope lsp_dynamic_workspace_symbols<CR>", { desc = "Open Workspace Symbols" }),
    buffer_map("n", "grt", "<cmd>Telescope lsp_type_definitions<CR>", { desc = "[G]oto [T]ype Definition" }),
    buffer_map(
      "n",
      "<leader>th",
      lsp.toggle_inlay_hints,
      { desc = "LSP: [T]oggle Inlay [H]ints", method = "textDocument/inlayHint" }
    ),
  },
  ---@type blink.cmp.KeymapConfig
  completion = {
    preset = "default",
    ["<CR>"] = { "accept", "fallback" },
    ["<Tab>"] = { "select_next", "snippet_forward", actions.shell_completion, "fallback" },
    ["<S-Tab>"] = { "select_prev", "snippet_backward", "fallback" },
  },
  neo_tree = { ["'"] = actions.previous_window },
  ---@type PickerKeys
  telescope = {
    terminal_opts = { nowait = true },
    switch = {
      files = { mode = { "i", "n" }, lhs = "<C-f>", desc = "Search files" },
      grep = { mode = { "i", "n" }, lhs = "<C-g>", desc = "Search by grep" },
    },
    close_terminal = {
      { mode = { "i", "n" }, lhs = "<Esc>", desc = "Close terminal search" },
      { mode = "i", lhs = "<C-c>", desc = "Close terminal search" },
      { mode = "n", lhs = "q", desc = "Close terminal search" },
    },
  },
  groups = {
    { "<leader>s", group = "[S]earch", mode = { "n", "v" } },
    { "<leader>t", group = "[T]oggle" },
    { "<leader>g", group = "[G]it" },
    { "<leader>r", group = "[R]efactor / Restart" },
    { "<leader>l", group = "[L]azy" },
    { "<leader>w", group = "[W]indow Navigation" },
    { "gr", group = "LSP Actions", mode = { "n" } },
  },
}

return M
