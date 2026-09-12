local root = vim.fn.getcwd()
vim.opt.rtp:prepend(root .. "/shared/nvim")
for _, name in ipairs { "nvim-lint", "conform.nvim" } do
  local plugin = vim.fn.stdpath "data" .. "/lazy/" .. name
  assert(vim.uv.fs_stat(plugin), "Missing Neovim test dependency: " .. name)
  vim.opt.rtp:append(plugin)
end
assert(vim.fn.executable "shuck" == 1, "Install shuck before running shell integration tests")
vim.env.NVIM_MINIMAL = nil
vim.cmd "filetype plugin indent on"
require "core.diagnostics"
require("plugins.lint").config()
require("conform").setup(require "languages.formatters")
vim.env.PATH = vim.env.PATH .. ":" .. vim.fn.stdpath "data" .. "/mason/bin"
assert(vim.fn.executable "bash-language-server" == 1, "Install bash-language-server before running shell tests")
for _, name in ipairs { "shuck", "bashls" } do
  vim.lsp.config(name, require("languages.servers").configs()[name])
  vim.lsp.enable(name)
end

local notices = {}
vim.notify = function(message)
  table.insert(notices, message)
end
local dir = vim.fn.tempname()
vim.fn.mkdir(dir, "p")
vim.fn.writefile({ "[format]", 'indent-style = "space"', "indent-width = 2" }, dir .. "/.shuck.toml")
local function diagnostics()
  return vim.tbl_filter(function(d)
    return d.source == "shuck"
  end, vim.diagnostic.get(0))
end
local function await(predicate)
  assert(vim.wait(5000, predicate, 20), vim.inspect { diagnostics = vim.diagnostic.get(0), notices = notices })
end
local function edit(name, lines)
  vim.fn.writefile(lines, dir .. "/" .. name)
  vim.cmd.edit(vim.fn.fnameescape(dir .. "/" .. name))
  await(function()
    local clients = vim.lsp.get_clients { bufnr = 0 }
    return #clients == 2 and clients[1].initialized and clients[2].initialized
  end)
end
local function replace(lines)
  vim.api.nvim_buf_set_lines(0, 0, -1, false, lines)
  vim.api.nvim_exec_autocmds("TextChanged", { buffer = 0 })
end

local editor
local ok, err = xpcall(function()
  edit(".zshrc", { "if then" })
  await(function()
    return #diagnostics() > 0
  end)
  assert(diagnostics()[1].severity == vim.diagnostic.severity.ERROR)
  assert(diagnostics()[1].lnum == 0)
  for _, diagnostic in ipairs(vim.diagnostic.get(0)) do
    assert(diagnostic.source == "shuck", "A second server published shell diagnostics")
  end
  local marks = vim.api.nvim_buf_get_extmarks(0, -1, 0, -1, { details = true })
  assert(
    vim.iter(marks):any(function(mark)
      return mark[4].virt_text ~= nil
    end),
    "Syntax error has no inline text"
  )
  vim.cmd.write()
  assert(vim.deep_equal(vim.fn.readfile(dir .. "/.zshrc"), { "if then" }), "Invalid save changed text")
  require("languages.shell").format { async = false }
  assert(vim.deep_equal(vim.api.nvim_buf_get_lines(0, 0, -1, false), { "if then" }))

  replace { "echo ok" }
  await(function()
    return #vim.diagnostic.get(0) == 0
  end)
  vim.cmd.write()

  -- Zsh-specific syntax must survive both save and manual formatting.
  local original = {
    "for item in *(N); do",
    'echo "$item:$#" "${(q)PWD}"',
    "done",
    "{",
    "echo work",
    "} always {",
    "echo cleanup",
    "}",
  }
  edit("format.zsh", original)
  vim.cmd.write()
  local formatted = vim.api.nvim_buf_get_lines(0, 0, -1, false)
  assert(formatted[2] == '  echo "$item:$#" "${(q)PWD}"', vim.inspect(formatted))
  assert(vim.deep_equal(formatted, vim.fn.readfile(dir .. "/format.zsh")))
  local cli = vim
    .system({ "shuck", "format", "--no-cache", "--stdin-filename", dir .. "/format.zsh", "-" }, {
      text = true,
      stdin = table.concat(original, "\n") .. "\n",
      env = { SHUCK_EXPERIMENTAL = "1" },
    })
    :wait()
  assert(cli.code == 0, cli.stderr)
  assert(cli.stdout == table.concat(formatted, "\n") .. "\n", "CLI and LSP formatting differ")
  local syntax = vim
    .system({ "zsh", "--no-exec", "--no-rcs", "--no-globalrcs" }, {
      text = true,
      stdin = cli.stdout,
    })
    :wait()
  assert(syntax.code == 0, syntax.stderr)
  replace(original)
  require("languages.shell").format { async = false }
  assert(vim.deep_equal(formatted, vim.api.nvim_buf_get_lines(0, 0, -1, false)))
  vim.cmd.write()

  edit("broken.sh", { "#!/usr/bin/env bash", "if then" })
  await(function()
    return #diagnostics() > 0
  end)
  assert(diagnostics()[1].lnum == 1)
  replace { "#!/usr/bin/env bash", "echo ok" }
  await(function()
    return #diagnostics() == 0
  end)
  vim.cmd.write()

  edit("lint.zsh", { "local unused_value=hello" })
  await(function()
    return vim.iter(diagnostics()):any(function(d)
      return d.code == "C001"
    end)
  end)

  edit("helper.zsh", { "helper() { echo ok; }" })
  edit("main.zsh", { "source " .. dir .. "/helper.zsh", "helper" })
  local client = vim.lsp.get_clients({ bufnr = 0, name = "shuck" })[1]
  local definition = client:request_sync("textDocument/definition", {
    textDocument = { uri = vim.uri_from_bufnr(0) },
    position = { line = 1, character = 2 },
  }, 3000, 0)
  assert(definition and not definition.err, vim.inspect(definition))
  assert(vim.inspect(definition.result):find("helper.zsh", 1, true), vim.inspect(definition))
  -- Exercise the real Blink menu and keymaps in an editor with no buffer words.
  editor = vim.fn.jobstart({ "nvim", "--embed", "--headless", "-u", "NONE", "-i", "NONE", "-n" }, { rpc = true })
  assert(editor > 0, "Could not start completion test editor")
  local function remote(code, ...)
    return vim.rpcrequest(editor, "nvim_exec_lua", code, { ... })
  end
  remote(
    [[
    local root = ...
    vim.opt.rtp:prepend(root .. "/shared/nvim")
    for _, name in ipairs { "blink.cmp", "LuaSnip" } do
      vim.opt.rtp:append(vim.fn.stdpath("data") .. "/lazy/" .. name)
    end
    vim.cmd("filetype plugin on")
    require("blink.cmp").setup(require("plugins.blink-cmp").opts)
    for _, name in ipairs { "shuck", "bashls" } do
      vim.lsp.config(name, require("languages.servers").configs()[name])
      vim.lsp.enable(name)
    end
  ]],
    root
  )
  local function has_item(label)
    return remote(
      [[
      local label = ...
      return vim.iter(require("blink.cmp").get_items()):any(function(item)
        local client = item.client_id and vim.lsp.get_client_by_id(item.client_id)
        return item.label == label and item.source_id == "lsp" and client and client.name == "bashls"
      end)
    ]],
      label
    )
  end
  local fixture = 0
  local function completion_buffer(ft, seed)
    fixture = fixture + 1
    remote(
      [[
      local path, ft, seed = ...
      table.insert(seed, "")
      vim.fn.writefile(seed, path)
      vim.cmd.edit { vim.fn.fnameescape(path), bang = true }
      vim.bo.filetype = ft
      vim.api.nvim_win_set_cursor(0, { #seed, 0 })
    ]],
      dir .. "/completion-" .. fixture .. "." .. ft,
      ft,
      seed or {}
    )
    assert(
      vim.wait(5000, function()
        return remote [[
        local clients = vim.lsp.get_clients({bufnr = 0})
        return #clients == 2 and clients[1].initialized and clients[2].initialized
      ]]
      end, 20),
      "Shell language servers did not attach"
    )
    remote [[
      local bufnr = vim.api.nvim_get_current_buf()
      local completion = vim.lsp.get_clients({bufnr = bufnr, method = "textDocument/completion"})
      assert(#completion == 1 and completion[1].name == "bashls", "Wrong completion provider")
      for _, method in ipairs { "textDocument/formatting", "textDocument/definition" } do
        local clients = vim.lsp.get_clients({bufnr = bufnr, method = method})
        assert(#clients == 1 and clients[1].name == "shuck", "Wrong provider for " .. method)
      end
    ]]
  end
  for _, ft in ipairs { "zsh", "sh" } do
    -- Even with a matching suggestion visible, Enter must preserve the typed text.
    completion_buffer(ft)
    vim.rpcrequest(editor, "nvim_input", "iunali")
    assert(
      vim.wait(3000, function()
        return has_item "unalias" and remote 'return require("blink.cmp").is_visible()'
      end, 20),
      "No visible suggestion for the Enter regression test"
    )
    vim.rpcrequest(editor, "nvim_input", "<CR>")
    assert(
      vim.wait(1000, function()
        return vim.deep_equal(remote [[return vim.api.nvim_buf_get_lines(0,0,-1,false)]], { "unali", "" })
      end, 20),
      "Enter accepted an unselected suggestion"
    )
    vim.rpcrequest(editor, "nvim_input", "<Esc>")

    completion_buffer(ft)
    vim.rpcrequest(editor, "nvim_input", "iexclude=1")
    assert(vim.wait(1000, function()
      return remote [[return vim.api.nvim_get_current_line()]] == "exclude=1"
    end, 20))
    vim.rpcrequest(editor, "nvim_input", "<CR>")
    assert(
      vim.wait(1000, function()
        return vim.deep_equal(remote [[return vim.api.nvim_buf_get_lines(0,0,-1,false)]], { "exclude=1", "" })
      end, 20),
      "Enter changed an assignment instead of starting a new line"
    )
    vim.rpcrequest(editor, "nvim_input", "<Esc>")
  end
  for _, case in ipairs {
    { ft = "zsh", prefix = "unali", label = "unalias" },
    { ft = "sh", prefix = "unali", label = "unalias" },
    { ft = "sh", prefix = "compge", label = "compgen" },
    { ft = "zsh", seed = { "project_helper() { echo ok; }" }, prefix = "project_h", label = "project_helper" },
    {
      ft = "zsh",
      seed = { "project_value=ok" },
      prefix = 'echo "$project_v',
      label = "project_value",
      expected = 'echo "$project_value',
    },
  } do
    completion_buffer(case.ft, case.seed)
    vim.rpcrequest(editor, "nvim_input", "i" .. case.prefix)
    assert(
      vim.wait(5000, function()
        return has_item(case.label)
      end, 20),
      "No automatic completion for " .. case.prefix .. " in " .. case.ft
    )
    remote 'require("blink.cmp").hide()'
    assert(vim.wait(1000, function()
      return not remote 'return require("blink.cmp").is_visible()'
    end, 20))
    vim.rpcrequest(editor, "nvim_input", "<Tab>")
    assert(
      vim.wait(3000, function()
        return has_item(case.label) and remote 'return require("blink.cmp").is_visible()'
      end, 20),
      "Tab did not open completion for " .. case.prefix
    )
    vim.rpcrequest(editor, "nvim_input", "<CR>")
    assert(
      vim.wait(1000, function()
        return vim.trim(remote "return vim.api.nvim_get_current_line()") == (case.expected or case.label)
      end, 20),
      "Completion was not accepted for "
        .. case.prefix
        .. ": "
        .. vim.inspect(remote [[return vim.api.nvim_buf_get_lines(0,0,-1,false)]])
    )
    vim.rpcrequest(editor, "nvim_input", "<Esc>")
  end
  for _, notice in ipairs(notices) do
    assert(not notice:find("ConformInfo", 1, true), notice)
  end
end, debug.traceback)
if editor then
  vim.fn.jobstop(editor)
end
vim.lsp.stop_client(vim.lsp.get_clients(), true)
vim.fn.delete(dir, "rf")
if not ok then
  error(err)
end
print "Shuck shell integration: passed"
