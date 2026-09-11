use super::*;

#[test]
fn command_tree_is_consistent() {
    Cli::command().debug_assert();
}
