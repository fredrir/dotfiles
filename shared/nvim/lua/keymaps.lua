local map = vim.keymap.set

-- General --

map("n", "<Esc>", "<cmd>nohlsearch<CR>", { desc = "Clear search highlight" })

-- [N]eovim --

map("n", "<leader>nr", "<cmd>NeovimRestart<CR>", { desc = "Neovim Restart" })
map("n", "<leader>rr", "<cmd>NeovimRestart<CR>", { desc = "Restart Neovim" })
map("n", "<leader>nq", "<cmd>NeovimClose<CR>", { desc = "Neovim Close" })
map("n", "<leader>ns", "<cmd>Lazy sync<CR>", { desc = "Neovim Sync" })
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

map("n", "<leader>w<Left>", "<cmd>leftabove vnew<CR>", { desc = "Split left" })
map("n", "<leader>w<Right>", "<cmd>rightbelow vnew<CR>", { desc = "Split right" })
map("n", "<leader>w<Up>", "<cmd>leftabove new<CR>", { desc = "Split up" })
map("n", "<leader>w<Down>", "<cmd>rightbelow new<CR>", { desc = "Split down" })
map("n", "<leader>wq", "<cmd>close<CR>", { desc = "Close current window" })

-- File explorers --

map("n", "<leader>e", "<cmd>Neotree toggle<CR>", { desc = "File [E]xplorer" })
map("n", "'", "<cmd>Neotree focus<CR>", { desc = "Focus NeoTree" })
map("n", "-", "<cmd>Oil<CR>", { desc = "Open parent directory" })

-- Diagnostics --

map("n", "<leader>q", "<cmd>lua vim.diagnostic.setloclist()<CR>", { desc = "Open diagnostic [Q]uickfix list" })
map("n", "<leader>d", "<cmd>Trouble diagnostics toggle<CR>", { desc = "Diagnostics (Trouble)" })

-- Formatting --

map({ "n", "v" }, "<leader>f", "<cmd>Format<CR>", { desc = "[F]ormat buffer" })

-- Search --

map("n", "<leader>sh", "<cmd>Telescope help_tags<CR>", { desc = "[S]earch [H]elp" })
map("n", "<leader>sk", "<cmd>Telescope keymaps<CR>", { desc = "[S]earch [K]eymaps" })
map("n", "<leader>sf", "<cmd>SearchFiles<CR>", { desc = "[S]earch [F]iles" })
map("n", "<leader>ss", "<cmd>Telescope builtin<CR>", { desc = "[S]earch [S]elect Telescope" })
map({ "n", "v" }, "<leader>sw", "<cmd>Telescope grep_string<CR>", { desc = "[S]earch current [W]ord" })
map("n", "<leader>sg", "<cmd>SearchGrep<CR>", { desc = "[S]earch by [G]rep" })
map("n", "<leader>sd", "<cmd>Telescope diagnostics<CR>", { desc = "[S]earch [D]iagnostics" })
map("n", "<leader>sr", "<cmd>Telescope resume<CR>", { desc = "[S]earch [R]esume" })
map("n", "<leader>s.", "<cmd>Telescope oldfiles<CR>", { desc = '[S]earch Recent Files ("." for repeat)' })
map("n", "<leader>sc", "<cmd>Telescope commands<CR>", { desc = "[S]earch [C]ommands" })
map("n", "<leader><leader>", "<cmd>Telescope buffers<CR>", { desc = "[ ] Find existing buffers" })
map("n", "<leader>/", "<cmd>SearchBuffer<CR>", { desc = "[/] Fuzzily search in current buffer" })
map("n", "<leader>s/", "<cmd>SearchGrepOpen<CR>", { desc = "[S]earch [/] in Open Files" })
map("n", "<leader>sn", "<cmd>SearchConfig<CR>", { desc = "[S]earch [N]eovim files" })

-- Git --

map("n", "<leader>g", "<cmd>Lazygit<CR>", { desc = "Lazygit" })

-- Harpoon --

map("n", "<leader>a", "<cmd>HarpoonAdd<CR>", { desc = "Harpoon: [A]dd file" })
map("n", "<C-e>", "<cmd>HarpoonMenu<CR>", { desc = "Harpoon: Quick menu" })
map("n", "<leader>1", "<cmd>HarpoonSelect 1<CR>", { desc = "Harpoon file 1" })
map("n", "<leader>2", "<cmd>HarpoonSelect 2<CR>", { desc = "Harpoon file 2" })
map("n", "<leader>3", "<cmd>HarpoonSelect 3<CR>", { desc = "Harpoon file 3" })
map("n", "<leader>4", "<cmd>HarpoonSelect 4<CR>", { desc = "Harpoon file 4" })

-- Terminal --

map("n", "<C-\\>", "<cmd>ToggleTerm<CR>", { desc = "Toggle terminal" })
map("t", "<C-\\>", "<cmd>ToggleTerm<CR>", { desc = "Toggle terminal" })
map("t", "<Esc><Esc>", "<C-\\><C-n>", { desc = "Exit terminal mode" })
