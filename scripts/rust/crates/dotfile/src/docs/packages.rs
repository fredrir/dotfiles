use crate::artifacts::packages;
use crate::context::Context;

pub(crate) fn group(name: &str, packages: &[(String, &str)]) -> String {
    let mut document = format!("\n## `{name}`\n\n");
    for (package, description) in packages {
        document.push_str(&format!("- `{package}`"));
        if !description.is_empty() {
            document.push_str(&format!(" — {description}"));
        }
        document.push('\n');
    }
    document
}

pub(super) fn outputs(
    context: &Context,
    inventory: bool,
) -> Result<Vec<super::plan::Output>, String> {
    let groups = packages::package_groups(context)?;
    packages::validate_packages(context, &groups)?;
    let metadata = packages::load_metadata(&context.packages_config)?;
    let (config, document) = packages::render(context, &groups, &metadata)?;
    let relative = context
        .packages_doc
        .strip_prefix(&context.root)
        .map_err(|e| e.to_string())?;
    let mut outputs = vec![super::plan::Output::text(relative, document)];
    if inventory {
        outputs.push(super::plan::Output::text(
            context
                .packages_config
                .strip_prefix(&context.root)
                .map_err(|e| e.to_string())?,
            config,
        ));
    }
    Ok(outputs)
}
