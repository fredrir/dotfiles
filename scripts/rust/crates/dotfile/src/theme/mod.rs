mod cli;
mod color;
mod emitters;
mod expression;
mod model;
mod plan;
mod preview;
mod render;
mod resolve;
mod selection;
mod validate;
pub use cli::{Args, run};
type Result<T> = std::result::Result<T, String>;
pub fn scopes(context: &crate::context::Context) -> Result<Vec<String>> {
    let repo = model::Repository::load(&context.root)?;
    let targets = emitters::targets(&repo)?;
    let inventory = selection::inventory(&targets);
    let mut values = vec!["global".into()];
    for (group, packages) in inventory {
        values.push(group.clone());
        values.extend(packages.into_iter().map(|p| format!("{group}/{p}")));
    }
    Ok(values)
}
pub fn profiles(context: &crate::context::Context) -> Result<Vec<String>> {
    model::profile_names(&context.root)
}
#[cfg(test)]
#[path = "../../tests/unit/theme_tests.rs"]
mod tests;
