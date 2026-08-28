if vim.g.minimal then
	return {}
end -- skipped in minimal (server) mode
return {
	"stevearc/conform.nvim",
	event = { "BufWritePre" },
	cmd = { "ConformInfo" },
	keys = {
		{
			"<leader>f",
			function()
				require("conform").format({ async = true, lsp_format = "fallback" })
			end,
			mode = "",
			desc = "[F]ormat buffer",
		},
	},
	-- Nothing detects these otherwise: neovim has no `dotfile` filetype, and
	-- `.config` reads as plain text. `.conf` keeps its own filetype and is
	-- pointed at dotfmt below.
	init = function()
		vim.filetype.add({ extension = { config = "dotfile", dotfile = "dotfile" } })
	end,
	---@module 'conform'
	---@type conform.setupOpts
	opts = {
		notify_on_error = false,
		format_on_save = function(bufnr)
			local disable_filetypes = { c = true, cpp = true }
			if disable_filetypes[vim.bo[bufnr].filetype] then
				return nil
			end
			return { timeout_ms = 500, lsp_format = "fallback" }
		end,
		formatters_by_ft = {
			lua = { "stylua" },
			python = { "ruff_format" },
			go = { "goimports", "gofmt" },
			javascript = { "biome" },
			typescript = { "biome" },
			javascriptreact = { "biome" },
			typescriptreact = { "biome" },
			css = { "biome" },
			html = { "biome" },
			json = { "biome" },
			jsonc = { "biome" },
			yaml = { "yamlfmt" },
			sql = { "sqlfluff" },
			toml = { "taplo" },
			conf = { "dotfmt" },
			dotfile = { "dotfmt" },
		},
		formatters = {
			-- Every diagnostic dotfmt has goes to stderr, so stdout is the buffer
			-- and nothing else; on a parse error it writes none of it and exits
			-- non-zero, which notify_on_error = false above turns into a no-op.
			dotfmt = {
				command = "dotfmt",
				args = { "--stdin", "$FILENAME" },
				stdin = true,
			},
		},
	},
}
