return {
  'akinsho/toggleterm.nvim',
  version = '*',
  keys = {
    { '<C-\\>', '<cmd>ToggleTerm<CR>', desc = 'Toggle terminal' },
    { '<C-\\>', '<cmd>ToggleTerm<CR>', mode = 't', desc = 'Toggle terminal' },
  },
  config = function()
    require('toggleterm').setup {
      size = function(term)
        if term.direction == 'horizontal' then
          return 15
        elseif term.direction == 'vertical' then
          return vim.o.columns * 0.4
        end
      end,
      shade_terminals = true,
      shading_factor = -30,
      direction = 'float',
      float_opts = {
        border = 'curved',
        width = function() return math.floor(vim.o.columns * 0.85) end,
        height = function() return math.floor(vim.o.lines * 0.8) end,
        winblend = 8,
      },
      highlights = {
        FloatBorder = { link = 'FloatBorder' },
      },
    }

    -- Easy escape from terminal mode
    vim.keymap.set('t', '<Esc><Esc>', '<C-\\><C-n>', { desc = 'Exit terminal mode' })

    -- Navigation out of the terminal into other windows
    vim.keymap.set('t', '<C-h>', '<C-\\><C-n><C-w>h', { desc = 'Move to left window' })
    vim.keymap.set('t', '<C-j>', '<C-\\><C-n><C-w>j', { desc = 'Move to lower window' })
    vim.keymap.set('t', '<C-k>', '<C-\\><C-n><C-w>k', { desc = 'Move to upper window' })
    vim.keymap.set('t', '<C-l>', '<C-\\><C-n><C-w>l', { desc = 'Move to right window' })
  end,
}
