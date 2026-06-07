return {
  {
    'kristijanhusak/vim-dadbod-ui',
    dependencies = {
      { 'tpope/vim-dadbod', lazy = true },
      {
        'kristijanhusak/vim-dadbod-completion',
        ft = { 'sql', 'mysql', 'plsql' },
        lazy = true,
      },
    },
    cmd = {
      'DBUI',
      'DBUIToggle',
      'DBUIAddConnection',
      'DBUIFindBuffer',
    },
    keys = {
      { '<leader>db', '<cmd>DBUIToggle<cr>', desc = 'Toggle DB UI' },
    },
    init = function()
      vim.g.db_ui_use_nerd_fonts = 1
      vim.g.db_ui_save_location = vim.fn.stdpath 'data' .. '/dadbod_ui'

      vim.g.dbs = {
        pyparser_llunde = 'postgres://pyparser:pyparser@localhost:5433/pyparser_llunde',
      }
    end,
  },
}
