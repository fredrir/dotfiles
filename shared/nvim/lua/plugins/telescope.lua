return {
  'nvim-telescope/telescope.nvim',
  event = 'VimEnter',
  cmd = { 'Telescope', 'TerminalSearch' },
  dependencies = {
    'nvim-lua/plenary.nvim',
    {
      'nvim-telescope/telescope-fzf-native.nvim',
      build = 'make',
      cond = function() return vim.fn.executable 'make' == 1 end,
    },
    { 'nvim-telescope/telescope-ui-select.nvim' },
    { 'nvim-tree/nvim-web-devicons', enabled = vim.g.have_nerd_font },
  },
  config = function()
    require('telescope').setup {
      extensions = {
        ['ui-select'] = { require('telescope.themes').get_dropdown() },
      },
    }

    pcall(require('telescope').load_extension, 'fzf')
    pcall(require('telescope').load_extension, 'ui-select')

    local builtin = require 'telescope.builtin'
    local actions = require 'telescope.actions'
    local action_state = require 'telescope.actions.state'
    local search_files
    local search_grep

    local function switch_to(search, terminal)
      return function(prompt_bufnr)
        local prompt = action_state.get_current_line()
        actions.close(prompt_bufnr)
        vim.schedule(function() search { default_text = prompt, terminal = terminal } end)
      end
    end

    local function search_mappings(terminal)
      return function(prompt_bufnr, map)
        map('i', '<C-f>', switch_to(search_files, terminal))
        map('n', '<C-f>', switch_to(search_files, terminal))
        map('i', '<C-g>', switch_to(search_grep, terminal))
        map('n', '<C-g>', switch_to(search_grep, terminal))

        if terminal then
          local function quit()
            actions.close(prompt_bufnr)
            vim.schedule(function() vim.cmd 'qall!' end)
          end

          local picker = action_state.get_current_picker(prompt_bufnr)

          for _, bufnr in ipairs { picker.prompt_bufnr, picker.results_bufnr, picker.preview_bufnr } do
            if bufnr and vim.api.nvim_buf_is_valid(bufnr) then
              vim.keymap.set({ 'i', 'n' }, '<Esc>', quit, { buffer = bufnr, nowait = true })
              vim.keymap.set('i', '<C-c>', quit, { buffer = bufnr, nowait = true })
              vim.keymap.set('n', 'q', quit, { buffer = bufnr, nowait = true })
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

    local function picker_options(opts, prompt_title)
      local terminal = opts and opts.terminal
      local options = vim.tbl_deep_extend('force', {
        prompt_title = prompt_title,
        attach_mappings = search_mappings(terminal),
      }, opts or {})
      options.terminal = nil
      return options
    end

    search_files = function(opts)
      builtin.find_files(picker_options(opts, 'Files  <C-g> Grep'))
    end

    search_grep = function(opts)
      builtin.live_grep(picker_options(opts, 'Grep  <C-f> Files'))
    end

    vim.api.nvim_create_user_command('TerminalSearch', function()
      vim.schedule(function() search_files { terminal = true } end)
    end, {})
    vim.keymap.set('n', '<leader>sh', builtin.help_tags, { desc = '[S]earch [H]elp' })
    vim.keymap.set('n', '<leader>sk', builtin.keymaps, { desc = '[S]earch [K]eymaps' })
    vim.keymap.set('n', '<leader>sf', search_files, { desc = '[S]earch [F]iles' })
    vim.keymap.set('n', '<leader>ss', builtin.builtin, { desc = '[S]earch [S]elect Telescope' })
    vim.keymap.set({ 'n', 'v' }, '<leader>sw', builtin.grep_string, { desc = '[S]earch current [W]ord' })
    vim.keymap.set('n', '<leader>sg', search_grep, { desc = '[S]earch by [G]rep' })
    vim.keymap.set('n', '<leader>sd', builtin.diagnostics, { desc = '[S]earch [D]iagnostics' })
    vim.keymap.set('n', '<leader>sr', builtin.resume, { desc = '[S]earch [R]esume' })
    vim.keymap.set('n', '<leader>s.', builtin.oldfiles, { desc = '[S]earch Recent Files ("." for repeat)' })
    vim.keymap.set('n', '<leader>sc', builtin.commands, { desc = '[S]earch [C]ommands' })
    vim.keymap.set('n', '<leader><leader>', builtin.buffers, { desc = '[ ] Find existing buffers' })

    vim.api.nvim_create_autocmd('LspAttach', {
      group = vim.api.nvim_create_augroup('telescope-lsp-attach', { clear = true }),
      callback = function(event)
        local buf = event.buf
        vim.keymap.set('n', 'grr', builtin.lsp_references, { buffer = buf, desc = '[G]oto [R]eferences' })
        vim.keymap.set('n', 'gri', builtin.lsp_implementations, { buffer = buf, desc = '[G]oto [I]mplementation' })
        vim.keymap.set('n', 'grd', builtin.lsp_definitions, { buffer = buf, desc = '[G]oto [D]efinition' })
        vim.keymap.set('n', 'gO', builtin.lsp_document_symbols, { buffer = buf, desc = 'Open Document Symbols' })
        vim.keymap.set('n', 'gW', builtin.lsp_dynamic_workspace_symbols, { buffer = buf, desc = 'Open Workspace Symbols' })
        vim.keymap.set('n', 'grt', builtin.lsp_type_definitions, { buffer = buf, desc = '[G]oto [T]ype Definition' })
      end,
    })

    vim.keymap.set('n', '<leader>/', function()
      builtin.current_buffer_fuzzy_find(require('telescope.themes').get_dropdown {
        winblend = 10,
        previewer = false,
      })
    end, { desc = '[/] Fuzzily search in current buffer' })

    vim.keymap.set('n', '<leader>s/', function()
      builtin.live_grep {
        grep_open_files = true,
        prompt_title = 'Live Grep in Open Files',
      }
    end, { desc = '[S]earch [/] in Open Files' })

    vim.keymap.set('n', '<leader>sn', function() builtin.find_files { cwd = vim.fn.stdpath 'config' } end, { desc = '[S]earch [N]eovim files' })
  end,
}
