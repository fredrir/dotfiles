local M = {}
local map = vim.keymap.set
local profile = require "core.profile"
local commands = require "core.commands"

-- General --

map("n", "<Esc>", "<cmd>nohlsearch<CR>")

-- Neovim --

map("n", "<leader>rr", commands.restart, { desc = "Restart Neovim" })

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

-- Files --

map("n", "<leader>e", "<cmd>Neotree toggle<CR>", { desc = "File [E]xplorer" })
map("n", "\\", "<cmd>Neotree reveal<CR>", { desc = "NeoTree reveal", silent = true })
map("n", "-", "<cmd>Oil<CR>", { desc = "Open parent directory" })

M.neo_tree_window = {
  ["\\"] = "close_window",
}

map("n", "'", "<cmd>Neotree toggle<CR>", {
  desc = "Toggle NeoTree",
  silent = true,
})

M.neo_tree_window["'"] = "close_window"

-- Navigation --

map("n", "<leader>n<Left>", "<cmd>leftabove vnew<CR>")
map("n", "<leader>n<Right>", "<cmd>rightbelow vnew<CR>")
map("n", "<leader>n<Up>", "<cmd>leftabove new<CR>")
map("n", "<leader>n<Down>", "<cmd>rightbelow new<CR>")

map("n", "<leader>nq", "<cmd>close<CR>", { desc = "Close current window" })

-- Diagnostics --

map("n", "<leader>q", vim.diagnostic.setloclist, { desc = "Open diagnostic [Q]uickfix list" })
map("n", "<leader>dd", "<cmd>Trouble diagnostics toggle<CR>", { desc = "Diagnostics (Trouble)" })

-- Formatting --

if not profile.minimal then
  map("", "<leader>f", function()
    require("conform").format { async = true, lsp_format = "fallback" }
  end, { desc = "[F]ormat buffer" })
end

-- Search --

M.telescope_prompt_titles = {
  files = "Files  <C-g> Grep",
  grep = "Grep  <C-f> Files",
}

---@param builtin table
---@param search_files fun(opts?: table)
---@param search_grep fun(opts?: table)
function M.telescope(builtin, search_files, search_grep)
  map("n", "<leader>sh", builtin.help_tags, { desc = "[S]earch [H]elp" })
  map("n", "<leader>sk", builtin.keymaps, { desc = "[S]earch [K]eymaps" })
  map("n", "<leader>sf", search_files, { desc = "[S]earch [F]iles" })
  map("n", "<leader>ss", builtin.builtin, { desc = "[S]earch [S]elect Telescope" })
  map({ "n", "v" }, "<leader>sw", builtin.grep_string, { desc = "[S]earch current [W]ord" })
  map("n", "<leader>sg", search_grep, { desc = "[S]earch by [G]rep" })
  map("n", "<leader>sd", builtin.diagnostics, { desc = "[S]earch [D]iagnostics" })
  map("n", "<leader>sr", builtin.resume, { desc = "[S]earch [R]esume" })
  map("n", "<leader>s.", builtin.oldfiles, { desc = '[S]earch Recent Files ("." for repeat)' })
  map("n", "<leader>sc", builtin.commands, { desc = "[S]earch [C]ommands" })
  map("n", "<leader><leader>", builtin.buffers, { desc = "[ ] Find existing buffers" })

  map("n", "<leader>/", function()
    builtin.current_buffer_fuzzy_find(require("telescope.themes").get_dropdown {
      winblend = 10,
      previewer = false,
    })
  end, { desc = "[/] Fuzzily search in current buffer" })

  map("n", "<leader>s/", function()
    builtin.live_grep {
      grep_open_files = true,
      prompt_title = "Live Grep in Open Files",
    }
  end, { desc = "[S]earch [/] in Open Files" })

  map("n", "<leader>sn", function()
    builtin.find_files { cwd = vim.fn.stdpath "config" }
  end, { desc = "[S]earch [N]eovim files" })
end

---@param opts table
---@return fun(prompt_bufnr: integer, picker_map: function): boolean
function M.telescope_picker(opts)
  return function(prompt_bufnr, picker_map)
    local function switch_to(search)
      return function()
        local prompt = opts.action_state.get_current_line()
        opts.actions.close(prompt_bufnr)
        vim.schedule(function()
          search { default_text = prompt, terminal = opts.terminal }
        end)
      end
    end

    picker_map("i", "<C-f>", switch_to(opts.search_files))
    picker_map("n", "<C-f>", switch_to(opts.search_files))
    picker_map("i", "<C-g>", switch_to(opts.search_grep))
    picker_map("n", "<C-g>", switch_to(opts.search_grep))

    if opts.terminal then
      local function quit()
        opts.actions.close(prompt_bufnr)
        vim.schedule(function()
          vim.cmd "qall!"
        end)
      end

      local picker = opts.action_state.get_current_picker(prompt_bufnr)
      for _, bufnr in ipairs { picker.prompt_bufnr, picker.results_bufnr, picker.preview_bufnr } do
        if bufnr and vim.api.nvim_buf_is_valid(bufnr) then
          map({ "i", "n" }, "<Esc>", quit, { buffer = bufnr, nowait = true })
          map("i", "<C-c>", quit, { buffer = bufnr, nowait = true })
          map("n", "q", quit, { buffer = bufnr, nowait = true })
        end
      end

      vim.schedule(function()
        if vim.api.nvim_win_is_valid(picker.prompt_win) then
          vim.api.nvim_set_current_win(picker.prompt_win)
        end
      end)
    end

    return true
  end
end

-- LSP --

---@param event table
---@param client vim.lsp.Client|nil
function M.lsp(event, client)
  local function buffer_map(mode, lhs, rhs, desc)
    map(mode, lhs, rhs, { buffer = event.buf, desc = desc })
  end

  local function telescope_builtin(name)
    return function()
      require("telescope.builtin")[name]()
    end
  end

  buffer_map("n", "grn", vim.lsp.buf.rename, "LSP: [R]e[n]ame")
  buffer_map("n", "<leader>rn", vim.lsp.buf.rename, "LSP: [R]e[n]ame")
  buffer_map({ "n", "x" }, "gra", vim.lsp.buf.code_action, "LSP: [G]oto Code [A]ction")
  buffer_map("n", "grD", vim.lsp.buf.declaration, "LSP: [G]oto [D]eclaration")
  buffer_map("n", "grr", telescope_builtin "lsp_references", "[G]oto [R]eferences")
  buffer_map("n", "gri", telescope_builtin "lsp_implementations", "[G]oto [I]mplementation")
  buffer_map("n", "grd", telescope_builtin "lsp_definitions", "[G]oto [D]efinition")
  buffer_map("n", "gO", telescope_builtin "lsp_document_symbols", "Open Document Symbols")
  buffer_map("n", "gW", telescope_builtin "lsp_dynamic_workspace_symbols", "Open Workspace Symbols")
  buffer_map("n", "grt", telescope_builtin "lsp_type_definitions", "[G]oto [T]ype Definition")

  if client and client:supports_method("textDocument/inlayHint", event.buf) then
    buffer_map("n", "<leader>th", function()
      vim.lsp.inlay_hint.enable(not vim.lsp.inlay_hint.is_enabled { bufnr = event.buf })
    end, "LSP: [T]oggle Inlay [H]ints")
  end
end

-- Git --

---@param bufnr integer
function M.gitsigns(bufnr)
  local gitsigns = require "gitsigns"

  local function buffer_map(mode, lhs, rhs, opts)
    opts = opts or {}
    opts.buffer = bufnr
    map(mode, lhs, rhs, opts)
  end

  buffer_map("n", "]c", function()
    if vim.wo.diff then
      vim.cmd.normal { "]c", bang = true }
    else
      gitsigns.nav_hunk "next"
    end
  end, { desc = "Jump to next git [c]hange" })

  buffer_map("n", "[c", function()
    if vim.wo.diff then
      vim.cmd.normal { "[c", bang = true }
    else
      gitsigns.nav_hunk "prev"
    end
  end, { desc = "Jump to previous git [c]hange" })

  buffer_map("v", "<leader>hs", function()
    gitsigns.stage_hunk { vim.fn.line ".", vim.fn.line "v" }
  end, { desc = "git [s]tage hunk" })
  buffer_map("v", "<leader>hr", function()
    gitsigns.reset_hunk { vim.fn.line ".", vim.fn.line "v" }
  end, { desc = "git [r]eset hunk" })
  buffer_map("n", "<leader>hs", gitsigns.stage_hunk, { desc = "git [s]tage hunk" })
  buffer_map("n", "<leader>hr", gitsigns.reset_hunk, { desc = "git [r]eset hunk" })
  buffer_map("n", "<leader>hS", gitsigns.stage_buffer, { desc = "git [S]tage buffer" })
  buffer_map("n", "<leader>hu", gitsigns.stage_hunk, { desc = "git [u]ndo stage hunk" })
  buffer_map("n", "<leader>hR", gitsigns.reset_buffer, { desc = "git [R]eset buffer" })
  buffer_map("n", "<leader>hp", gitsigns.preview_hunk, { desc = "git [p]review hunk" })
  buffer_map("n", "<leader>hb", gitsigns.blame_line, { desc = "git [b]lame line" })
  buffer_map("n", "<leader>hd", gitsigns.diffthis, { desc = "git [d]iff against index" })
  buffer_map("n", "<leader>hD", function()
    gitsigns.diffthis "@"
  end, { desc = "git [D]iff against last commit" })
  buffer_map("n", "<leader>tb", gitsigns.toggle_current_line_blame, { desc = "[T]oggle git show [b]lame line" })
  buffer_map("n", "<leader>tD", gitsigns.preview_hunk_inline, { desc = "[T]oggle git show [D]eleted" })
end

local lazygit
map("n", "<leader>gg", function()
  if not lazygit then
    local Terminal = require("toggleterm.terminal").Terminal
    lazygit = Terminal:new {
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
  end
  lazygit:toggle()
end, { desc = "Lazygit" })

-- Harpoon --

map("n", "<leader>a", function()
  require("harpoon"):list():add()
end, { desc = "Harpoon: [A]dd file" })
map("n", "<C-e>", function()
  local harpoon = require "harpoon"
  harpoon.ui:toggle_quick_menu(harpoon:list())
end, { desc = "Harpoon: Quick menu" })

for index = 1, 4 do
  local target = index
  map("n", "<leader>" .. target, function()
    require("harpoon"):list():select(target)
  end, { desc = "Harpoon file " .. target })
end

-- Debugging --

if not profile.minimal then
  map("n", "<F5>", function()
    require("dap").continue()
  end, { desc = "Debug: Start/Continue" })
  map("n", "<F1>", function()
    require("dap").step_into()
  end, { desc = "Debug: Step Into" })
  map("n", "<F2>", function()
    require("dap").step_over()
  end, { desc = "Debug: Step Over" })
  map("n", "<F3>", function()
    require("dap").step_out()
  end, { desc = "Debug: Step Out" })
  map("n", "<leader>b", function()
    require("dap").toggle_breakpoint()
  end, { desc = "Debug: Toggle Breakpoint" })
  map("n", "<leader>B", function()
    require("dap").set_breakpoint(vim.fn.input "Breakpoint condition: ")
  end, { desc = "Debug: Set Breakpoint" })
  map("n", "<F7>", function()
    require "dap"
    require("dapui").toggle()
  end, { desc = "Debug: See last session result." })
end

-- Terminal --

map("n", "<C-\\>", "<cmd>ToggleTerm<CR>", { desc = "Toggle terminal" })
map("t", "<C-\\>", "<cmd>ToggleTerm<CR>", { desc = "Toggle terminal" })
map("t", "<Esc><Esc>", "<C-\\><C-n>", { desc = "Exit terminal mode" })

-- Database --

if not profile.minimal then
  map("n", "<leader>db", "<cmd>DBUIToggle<CR>", { desc = "Toggle DB UI" })
end

-- Completion --

M.blink = { preset = "default" }

-- Which Key --
M.which_key_groups = {
  { "<leader>s", group = "[S]earch", mode = { "n", "v" } },
  { "<leader>t", group = "[T]oggle" },
  { "<leader>g", group = "[G]it" },
  { "<leader>h", group = "Git [H]unk", mode = { "n", "v" } },
  { "<leader>r", group = "[R]efactor / Restart" },
  { "<leader>n", group = "Window [N]avigation" },
  { "gr", group = "LSP Actions", mode = { "n" } },
}

return M
