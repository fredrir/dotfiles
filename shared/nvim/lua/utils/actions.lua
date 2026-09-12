local M = { git = {}, harpoon = {}, debug = {} }

---@class ActionOptions
---@field lazygit table
---@field shell_filetypes table<string, boolean>
---@field shell_show blink.cmp.ShowOpts
---@field minimum_keyword_length table<string, integer>

---@type ActionOptions
local options

---@param opts ActionOptions
function M.setup(opts)
  options = opts
end

---@type Terminal?
local lazygit

---@param direction "next"|"prev"
local function navigate_hunk(direction)
  if vim.wo.diff then
    vim.cmd.normal { direction == "next" and "]c" or "[c", bang = true }
    return
  end
  require("gitsigns").nav_hunk(direction)
end

function M.git.next_hunk()
  navigate_hunk "next"
end

function M.git.previous_hunk()
  navigate_hunk "prev"
end

function M.git.stage_selection()
  require("gitsigns").stage_hunk { vim.fn.line ".", vim.fn.line "v" }
end

function M.git.reset_selection()
  require("gitsigns").reset_hunk { vim.fn.line ".", vim.fn.line "v" }
end

function M.git.diff_last_commit()
  require("gitsigns").diffthis "@"
end

function M.git.lazygit()
  if not lazygit then
    lazygit = require("toggleterm.terminal").Terminal:new(options.lazygit)
  end
  lazygit:toggle()
end

function M.harpoon.add()
  require("harpoon"):list():add()
end

function M.harpoon.menu()
  local harpoon = require "harpoon"
  harpoon.ui:toggle_quick_menu(harpoon:list())
end

---@param index integer
---@return fun()
function M.harpoon.select(index)
  return function()
    require("harpoon"):list():select(index)
  end
end

function M.debug.continue()
  require("dap").continue()
end

function M.debug.step_into()
  require("dap").step_into()
end

function M.debug.step_over()
  require("dap").step_over()
end

function M.debug.step_out()
  require("dap").step_out()
end

function M.debug.toggle_breakpoint()
  require("dap").toggle_breakpoint()
end

function M.debug.conditional_breakpoint()
  require("dap").set_breakpoint(vim.fn.input "Breakpoint condition: ")
end

function M.debug.toggle_ui()
  require "dap"
  require("dapui").toggle()
end

function M.previous_window()
  vim.cmd "wincmd p"
end

---@param cmp blink.cmp.API
---@return boolean?
function M.shell_completion(cmp)
  if not options.shell_filetypes[vim.bo.filetype] then
    return
  end
  local before = vim.api.nvim_get_current_line():sub(1, vim.api.nvim_win_get_cursor(0)[2])
  if before:match "%S$" then
    return cmp.show(options.shell_show)
  end
end

---@param ctx blink.cmp.Context
---@return integer
function M.minimum_keyword_length(ctx)
  return options.minimum_keyword_length[vim.bo[ctx.bufnr].filetype] or options.minimum_keyword_length.default
end

return M
