use doc_keybinds::{MARKER, collect, generate};
use std::fs;
use std::path::Path;
use std::process::Command;

fn put(root: &Path, name: &str, body: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

#[test]
fn parses_all_formats_with_modes_descriptions_and_literal_loops() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    put(
        root,
        "shared/tmux/keys.conf",
        "set -g prefix C-b\n%if #{>=:#{version},3.4}\nbind -T copy-mode-vi -N 'Copy | selection' y send-keys -X copy-selection\n%else\nbind -n -r '-' send-keys \\\n 'literal # string'\n%endif\nunbind -q '%'\n",
    );
    put(
        root,
        "shared/zsh/conf.d/keys.zsh",
        "if command -v nvim >/dev/null; then\n bindkey -M viins '^F' search-files\nelse\n bindkey -M emacs $'\\e[13;2u' fallback\nfi\n",
    );
    put(
        root,
        "linux/hyprland/hypr/vars.conf",
        "$mainMod = SUPER\n$terminal = kitty\n",
    );
    put(
        root,
        "linux/hyprland/hypr/keys.conf",
        "submap = resize\nbindd = $mainMod SHIFT, E, Open terminal, exec, $terminal --title a,b\nbindm = $mainMod, mouse:272, movewindow\n",
    );
    put(
        root,
        "linux/kde/plasma/kglobalshortcutsrc",
        "[kwin]\n_k_friendly_name=KWin\nClose=Meta+Q\\tAlt+F4,Alt+F4,Close window\nDisabled=none,Meta+D,Old binding\n",
    );
    put(
        root,
        "shared/vscode/keybindings.json",
        "[\n// ignored\n{\"key\":\"ctrl+k\",\"command\":\"runCommands\",\"when\":\"editorFocus || terminalFocus\",\"args\":{\"text\":\"https://test/* hi */\"},},\n{\"key\":\"alt+x\",\"command\":\"-removeMe\"},\n]\n",
    );
    put(
        root,
        "shared/yazi/keymap.toml",
        "[[mgr.prepend_keymap]]\non = ['g', 'p']\nrun = ['cd ~/projects', 'reveal']\ndesc = 'Projects'\n",
    );
    put(
        root,
        "shared/nvim/lua/core/keymap.lua",
        r#"
local map = vim.keymap.set
for i = 1, 4 do
 local target = i
 map({ 'n', 'v' }, '<leader>' .. target, function() select(target) end, { desc = 'File ' .. target })
end
if not profile.minimal then
 map('n', '<F5>', function()
   require('dap').continue()
 end, { desc = 'Debug' })
end
function M.lsp(event)
 local function buffer_map(mode, lhs, rhs, desc)
   map(mode, lhs, rhs, {buffer=event.buf, desc=desc})
 end
 buffer_map('n', 'grn', vim.lsp.buf.rename, 'Rename')
end
for key, direction in pairs { h = 'left', l = 'right' } do
 vim.keymap.set({'n', 't'}, '<C-' .. key .. '>', splits['move_' .. direction], { desc = 'Move ' .. direction })
end
-- map('n', 'ignored', 'not-a-binding')
"#,
    );
    put(
        root,
        "shared/wezterm/keymap/modifiers.lua",
        "local MOD\nif platform.is_mac then MOD = {PRIMARY='CMD'} else MOD = {PRIMARY='CTRL'} end\nreturn MOD\n",
    );
    put(
        root,
        "shared/wezterm/keymap/init.lua",
        r#"
local MOD = require 'keymap.modifiers'
local physical = require 'keymap.physical'
local keys = {{key='c', mods=platform.is_mac and {'CMD', 'CTRL'} or 'CTRL|SHIFT', action=act.CopyTo 'Clipboard'}}
for i = 1, 9 do
 table.insert(keys, {key=tostring(i),mods=MOD.PRIMARY,action=act.ActivateTab(i-1)})
end
for key, direction in pairs { LeftArrow = {'h', 'Left'}, RightArrow = {'l', 'Right'} } do
 table.insert(keys, {key=key,mods=MOD.PRIMARY,action=act.ActivatePaneDirection(direction[2])})
end
if platform.is_mac then extend(keys, physical) end
"#,
    );
    put(
        root,
        "shared/wezterm/keymap/physical.lua",
        "return {{key='phys:8',mods='OPT',action=act.SendString '['}}\n",
    );
    let packages = collect(root).unwrap();
    assert_eq!(packages.len(), 8);
    assert_eq!(packages["tmux"].bindings.len(), 3);
    assert_eq!(packages["tmux"].settings[0].action, "C-b");
    assert!(
        packages["tmux"]
            .bindings
            .iter()
            .any(|b| b.description == "Copy | selection" && b.context.contains("copy-mode-vi"))
    );
    assert!(
        packages["tmux"]
            .bindings
            .iter()
            .any(|b| b.context.contains("not (")
                && b.action.contains("literal # string")
                && b.line == 5)
    );
    assert_eq!(packages["zsh"].bindings.len(), 2);
    assert!(
        packages["zsh"]
            .bindings
            .iter()
            .any(|b| b.key == "\\e[13;2u" && b.context.contains("not ("))
    );
    assert!(
        packages["hyprland"]
            .bindings
            .iter()
            .any(|b| b.key == "SUPER+SHIFT+E"
                && b.description == "Open terminal"
                && b.action == "exec, kitty --title a,b")
    );
    assert_eq!(packages["kde"].bindings.len(), 2);
    assert!(packages["kde"].bindings.iter().any(|b| b.key == "Unbound"));
    assert_eq!(packages["vscode"].bindings.len(), 2);
    assert!(
        packages["vscode"]
            .bindings
            .iter()
            .any(|b| b.line == 3 && b.action.contains("https://test/* hi */"))
    );
    assert_eq!(packages["yazi"].bindings[0].key, "g → p");
    assert_eq!(packages["nvim"].bindings.len(), 8);
    assert!(
        packages["nvim"]
            .bindings
            .iter()
            .any(|b| b.key == "<leader>4" && b.description == "File 4")
    );
    assert!(
        packages["nvim"]
            .bindings
            .iter()
            .any(|b| b.key == "<F5>" && b.context.contains("not profile.minimal"))
    );
    assert!(
        packages["nvim"]
            .bindings
            .iter()
            .any(|b| b.key == "grn" && b.description == "Rename" && b.context.contains("M.lsp"))
    );
    assert!(
        packages["nvim"]
            .bindings
            .iter()
            .any(|b| b.key == "<C-h>" && b.description == "Move left")
    );
    assert_eq!(packages["wezterm"].bindings.len(), 26);
    assert!(
        packages["wezterm"]
            .bindings
            .iter()
            .any(|b| b.key == "CMD+9" && b.action == "act.ActivateTab(8)")
    );
    assert!(packages["wezterm"].bindings.iter().any(|b| b.key == "CTRL+LeftArrow" && b.action == "act.ActivatePaneDirection(\"Left\")"));
    assert!(
        !packages["wezterm"]
            .bindings
            .iter()
            .any(|b| b.key == "OPT+phys:8" && b.context.contains("Linux"))
    );
}

#[test]
fn generation_is_deterministic_check_is_read_only_and_removed_sources_clear_rows() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    put(
        root,
        "shared/tmux/keys.conf",
        "bind -N 'A | <tag> & `tick`' '|' display-message hello\n",
    );
    assert_eq!(generate(root, true).unwrap().len(), 9);
    assert!(!root.join("docs").exists());
    assert_eq!(generate(root, false).unwrap().len(), 9);
    let path = root.join("docs/keybinds/tmux.md");
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    assert!(generate(root, false).unwrap().is_empty());
    assert_eq!(before, fs::metadata(&path).unwrap().modified().unwrap());
    let page = fs::read_to_string(&path).unwrap();
    assert!(page.contains("<code>&#124;</code>"));
    assert!(page.contains("A &#124; &lt;tag&gt; &amp; &#96;tick&#96;"));
    assert!(page.contains("../../shared/tmux/keys.conf#L1"));
    fs::remove_file(root.join("shared/tmux/keys.conf")).unwrap();
    assert_eq!(generate(root, true).unwrap().len(), 2);
    assert_eq!(fs::read_to_string(&path).unwrap(), page);
    generate(root, false).unwrap();
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("No configured bindings")
    );
    put(root, "docs/keybinds/old.md", &format!("{MARKER}\nold"));
    put(root, "docs/keybinds/manual.md", "handwritten");
    assert_eq!(
        generate(root, true).unwrap(),
        [Path::new("docs/keybinds/old.md")]
    );
    assert!(root.join("docs/keybinds/old.md").exists());
    generate(root, false).unwrap();
    assert!(!root.join("docs/keybinds/old.md").exists());
    assert!(root.join("docs/keybinds/manual.md").exists());
}

#[test]
fn invalid_input_aborts_before_writing_any_pages() {
    for (name, body) in [
        (
            "shared/nvim/lua/keys.lua",
            "vim.keymap.set('n', 'a', function( end)",
        ),
        ("shared/tmux/keys.conf", "bind -N 'broken x command"),
        ("shared/vscode/keybindings.json", "[{\"key\":\"a\"}]"),
        (
            "shared/yazi/keymap.toml",
            "[[mgr.prepend_keymap]]\non=['a']",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), name, body);
        let error = generate(dir.path(), false).unwrap_err();
        assert!(error.contains(name), "{error}");
        assert!(!dir.path().join("docs").exists());
    }
}

#[test]
fn unresolved_lua_keys_remain_visible_and_vendor_defaults_are_excluded() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "shared/nvim/lua/keys.lua",
        "vim.keymap.set('n', runtime_key(), 'action')\n",
    );
    put(
        dir.path(),
        "shared/yazi/plugins/vendor/keymap.toml",
        "invalid default",
    );
    let packages = collect(dir.path()).unwrap();
    assert_eq!(packages["nvim"].bindings[0].key, "runtime_key()");
    assert!(packages["nvim"].bindings[0].context.contains("unresolved"));
    assert!(packages["yazi"].bindings.is_empty());
}

#[test]
fn scalar_kde_launchers_and_zsh_completion_keys_are_documented() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "linux/kde/plasma/kglobalshortcutsrc",
        "[services][app.desktop]\n_launch=Meta+K\nCapture=Print\nUnused=\n",
    );
    put(
        dir.path(),
        "shared/zsh/conf.d/completion.zsh",
        "zstyle ':fzf-tab:*' switch-group '<' '>'\n",
    );
    let packages = collect(dir.path()).unwrap();
    assert_eq!(packages["kde"].bindings.len(), 3);
    assert!(
        packages["kde"]
            .bindings
            .iter()
            .any(|b| b.action == "_launch" && b.key == "Meta+K")
    );
    assert_eq!(packages["zsh"].bindings.len(), 2);
    assert!(
        packages["zsh"]
            .bindings
            .iter()
            .any(|b| b.key == "<" && b.action == "Previous completion group")
    );
}

#[test]
fn cli_check_reports_drift_and_finds_the_repository_from_a_child_directory() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "config/targets.dotfile", "");
    put(
        dir.path(),
        "shared/tmux/keys.conf",
        "bind r refresh-client\n",
    );
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_doc-keybinds"))
            .current_dir(dir.path().join("shared/tmux"))
            .env_remove("DOTFILE_ROOT")
            .args(args)
            .output()
            .unwrap()
    };
    assert_eq!(run(&["--check"]).status.code(), Some(1));
    assert!(!dir.path().join("docs").exists());
    assert!(run(&[]).status.success());
    assert!(run(&["--check"]).status.success());
    assert!(run(&["--command-dump"]).status.success());
    assert!(run(&["--completions", "zsh"]).status.success());
}
