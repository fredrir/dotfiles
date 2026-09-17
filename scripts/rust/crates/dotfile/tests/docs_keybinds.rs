#![forbid(unsafe_code)]

use dotfile_cli::docs::keybinds::{MARKER, collect};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn put(root: &Path, name: &str, body: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn generate(root: &Path, check: bool) -> Result<Vec<PathBuf>, String> {
    fs::create_dir_all(root.join("config")).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_dotfile"));
    command
        .env("DOTFILE_ROOT", root)
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("home/.config"))
        .args(["docs", "--only", "keybinds", "--json"]);
    if check {
        command.arg("--check");
    }
    let output = command.output().unwrap();
    if !output.status.success() && !(check && output.status.code() == Some(1)) {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| String::from_utf8_lossy(&output.stderr).into_owned())?;
    Ok(report["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| PathBuf::from(change["path"].as_str().unwrap()))
        .collect())
}

#[test]
fn parses_declarative_neovim_settings_and_picker_mappings() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    put(
        root,
        "shared/nvim/editor.lua",
        "return { globals = { mapleader = ' ', maplocalleader = ',' } }\n",
    );
    put(
        root,
        "shared/nvim/keymap.lua",
        "return {\n  files = { mode = {'i', 'n'}, lhs = '<C-f>', desc = 'Search files' },\n  close = { mode = 'n', lhs = 'q', rhs = 'quit', desc = 'Close search' },\n}\n",
    );
    let packages = collect(root).unwrap();
    let nvim = &packages["nvim"];
    assert_eq!(nvim.settings.len(), 2);
    assert!(
        nvim.settings
            .iter()
            .any(|s| s.key == "mapleader" && s.action == " ")
    );
    assert!(
        nvim.settings
            .iter()
            .any(|s| s.key == "maplocalleader" && s.action == ",")
    );
    assert_eq!(nvim.bindings.len(), 2);
    let files = nvim.bindings.iter().find(|b| b.key == "<C-f>").unwrap();
    assert_eq!(files.action, "Search files");
    assert_eq!(files.description, "Search files");
    assert!(files.context.contains("mode=i,n"));
    assert_eq!(files.line, 2);
    let close = nvim.bindings.iter().find(|b| b.key == "q").unwrap();
    assert_eq!(close.action, "quit");
    assert!(close.context.contains("mode=n"));
}

#[test]
fn parses_all_formats_with_modes_descriptions_and_literal_loops() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    put(
        root,
        "shared/zsh/60-keybinds.zsh",
        "if command -v nvim >/dev/null; then\n bindkey -M viins '^F' search-files\nelse\n bindkey -M emacs $'\\e[13;2u' fallback\nfi\nbindkey -N shift-select emacs\nbindkey -M shift-select -R ' '-'~' replace-region\n",
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
    assert_eq!(packages.len(), 7);
    assert_eq!(packages["zsh"].bindings.len(), 3);
    assert!(
        packages["zsh"]
            .settings
            .iter()
            .any(|s| s.key == "keymap" && s.action == "shift-select")
    );
    assert!(packages["zsh"].bindings.iter().any(|b| b.key == " -~"
        && b.action == "replace-region"
        && b.context.contains("shift-select")
        && b.context.contains("key range")));
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
        "shared/nvim/lua/core/keymap.lua",
        "local map = vim.keymap.set\nmap('n', '|', \"display-message '['\", { desc = \"A | <tag> & `tick`\" })\n",
    );
    assert_eq!(generate(root, true).unwrap().len(), 8);
    assert!(!root.join("docs").exists());
    assert_eq!(generate(root, false).unwrap().len(), 8);
    let path = root.join("docs/keybinds/nvim.md");
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    assert!(generate(root, false).unwrap().is_empty());
    assert_eq!(before, fs::metadata(&path).unwrap().modified().unwrap());
    let page = fs::read_to_string(&path).unwrap();
    assert!(page.contains("<code>&#124;</code>"));
    assert!(page.contains("A &#124; &lt;tag&gt; &amp; &#96;tick&#96;"));
    assert!(page.contains("../../shared/nvim/lua/core/keymap.lua#L2"));
    assert!(page.contains(
        "[<code>display-message '&#91;'</code>](../../shared/nvim/lua/core/keymap.lua#L2)"
    ));
    fs::remove_file(root.join("shared/nvim/lua/core/keymap.lua")).unwrap();
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
        ("shared/zsh/60-keybinds.zsh", "bindkey -M viins 'unclosed"),
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
fn discovery_prunes_unrelated_packages_and_preserves_overrides_and_linux_variants() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "shared/unrelated/nvim/invalid.lua",
        "not valid Lua (((",
    );
    put(
        dir.path(),
        "linux/custom/wezterm/keys.lua",
        "return {{key='a',mods='CTRL',action=act.Nop}}\n",
    );
    put(
        dir.path(),
        "linux/hyprland/overrides/laptop/hypr/keys.conf",
        "$mainMod = SUPER\nbind = $mainMod, Q, killactive\n",
    );
    put(
        dir.path(),
        "linux/kde/plasma/kglobalshortcutsrc",
        "[kwin]\nClose=Meta+Q,Alt+F4,Close window\n",
    );
    let packages = collect(dir.path()).unwrap();
    assert!(packages["nvim"].sources.is_empty());
    assert_eq!(
        packages["wezterm"].sources,
        ["linux/custom/wezterm/keys.lua"]
    );
    assert_eq!(
        packages["wezterm"]
            .bindings
            .iter()
            .map(|binding| binding.context.as_str())
            .collect::<Vec<_>>(),
        ["Linux/Windows", "macOS"]
    );
    assert_eq!(packages["hyprland"].bindings.len(), 1);
    assert_eq!(packages["kde"].bindings.len(), 1);
}

#[test]
fn cli_check_reports_drift_from_a_child_directory() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "config/targets.dotfile", "");
    put(
        dir.path(),
        "shared/zsh/60-keybinds.zsh",
        "bindkey -M viins '^F' search-files\n",
    );
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_dotfile"))
            .current_dir(dir.path().join("shared/zsh"))
            .env("DOTFILE_ROOT", dir.path())
            .args(["docs", "--only", "keybinds"])
            .args(args)
            .output()
            .unwrap()
    };
    assert_eq!(run(&["--check"]).status.code(), Some(1));
    assert!(!dir.path().join("docs").exists());
    assert!(run(&[]).status.success());
    assert!(run(&["--check"]).status.success());
}

#[test]
fn platform_tables_share_only_identical_bindings_and_keep_source_links() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "shared/wezterm/keymap/init.lua",
        r#"
local platform = require 'utils.platform'
local keys = {
 {key='Tab', mods='CTRL', action=act.ActivateTabRelative(1)},
 {key='x', mods=platform.is_mac and 'CMD' or 'CTRL', action=act.CloseCurrentTab},
 {key='y', mods='ALT', action=platform.is_mac and act.CopyTo('Clipboard') or act.PasteFrom('Clipboard')},
}
if platform.is_mac then
 if feature.mac then table.insert(keys, {key='s', mods='ALT', action=act.Nop}) end
 table.insert(keys, {key='Backspace', mods='CTRL', action=act.Nop})
else
 if feature.linux then table.insert(keys, {key='s', mods='ALT', action=act.Nop}) end
end
return {keys=keys, disable_default_key_bindings=true}
"#,
    );
    generate(dir.path(), false).unwrap();
    let page = fs::read_to_string(dir.path().join("docs/keybinds/wezterm.md")).unwrap();
    assert!(page.starts_with("# WezTerm Keybinds\n"));
    assert!(page.contains("| Shared | [Shared Keybinds](#shared-keybinds) |"));
    assert!(page.contains("| Mac | [Mac Keybinds](#mac-keybinds) |"));
    assert!(
        page.contains("| Linux / Windows | [Linux / Windows Keybinds](#linux--windows-keybinds) |")
    );
    assert_eq!(page.matches("| Key | Action | Description |").count(), 3);
    assert!(!page.contains("| Context |"));
    assert!(!page.contains("| Source |"));
    assert!(!page.contains("| Setting |"));
    let section = |name: &str| {
        page.split(&format!("## {name} Keybinds\n"))
            .nth(1)
            .unwrap()
            .split("\n## ")
            .next()
            .unwrap()
    };
    let shared = section("Shared");
    let mac = section("Mac");
    let linux = section("Linux / Windows");
    assert_eq!(page.matches("<code>CTRL+Tab</code>").count(), 1);
    assert!(shared.contains("<code>CTRL+Tab</code>"));
    assert!(shared.contains("disable_default_key_bindings = true"));
    assert!(shared.contains(
        "[<code>act.ActivateTabRelative(1)</code>](../../shared/wezterm/keymap/init.lua#L4)"
    ));
    assert!(!shared.contains("<code>ALT+y</code>"));
    assert!(mac.contains("<code>CMD+x</code>"));
    assert!(linux.contains("<code>CTRL+x</code>"));
    assert!(mac.contains("<code>ALT+y</code>"));
    assert!(linux.contains("<code>ALT+y</code>"));
    assert!(mac.contains("<code>CTRL+Backspace</code>"));
    assert!(!linux.contains("<code>CTRL+Backspace</code>"));
    assert!(!shared.contains("<code>ALT+s</code>"));
    assert!(mac.contains("<code>feature.mac</code>"));
    assert!(linux.contains("<code>feature.linux</code>"));
    assert!(generate(dir.path(), true).unwrap().is_empty());
}

#[test]
fn platform_sections_follow_source_directories_and_mac_command_keys() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "shared/vscode/keybindings.json",
        r#"[
        {"key":"cmd+x", "command":"macCommand"},
        {"key":"ctrl+x", "command":"sharedCommand", "when":"editorFocus"}
    ]"#,
    );
    put(
        dir.path(),
        "macos/vscode/keybindings.json",
        r#"[{"key":"ctrl+k", "command":"macOverride"}]"#,
    );
    put(
        dir.path(),
        "linux/hyprland/hypr/keys.conf",
        "bind = SUPER, Q, killactive\n",
    );
    generate(dir.path(), false).unwrap();
    let page = fs::read_to_string(dir.path().join("docs/keybinds/vscode.md")).unwrap();
    let (shared, mac) = page.split_once("## Mac Keybinds").unwrap();
    assert!(shared.contains("<code>ctrl+x</code>"));
    assert!(shared.contains("<code>editorFocus</code>"));
    assert!(!shared.contains("<code>cmd+x</code>"));
    assert!(mac.contains("<code>cmd+x</code>"));
    assert!(mac.contains("<code>ctrl+k</code>"));
    let linux = fs::read_to_string(dir.path().join("docs/keybinds/hyprland.md")).unwrap();
    assert!(linux.contains("| Linux | [Linux Keybinds](#linux-keybinds) |"));
    assert!(!linux.contains("Windows"));
    assert!(!linux.contains("## Shared Keybinds"));
}
