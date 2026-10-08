use std::path::Path;
use testkit::Bin;

pub fn dotfmt(root: &Path) -> Bin {
    Bin::new(env!("CARGO_BIN_EXE_dotfmt"))
        .plain()
        .env("PATH", "")
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join(".config"))
        .env("DOTFILE_ROOT", "/no/checkout/needed")
        .current_dir(root)
}
