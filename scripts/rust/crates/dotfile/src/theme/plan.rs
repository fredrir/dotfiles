use super::{
    Result,
    emitters::{self, Target},
    model::Repository,
    selection::Selection,
};
use std::fs;
#[derive(Debug)]
pub struct Change {
    pub path: String,
    pub content: String,
}
pub fn generate(
    repo: &Repository,
    targets: &[Target],
    selection: &Selection,
) -> Result<Vec<Change>> {
    super::validate::all(repo)?;
    let mut changes = Vec::new();
    for target in targets {
        crate::cancel::check()?;
        let content = emitters::emit(repo, repo.theme(selection.for_path(&target.path))?, target)?;
        let old = match fs::read(repo.root.join(&target.path)) {
            Ok(content) => Some(content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("{}: {e}", target.path)),
        };
        if old.as_deref() != Some(content.as_bytes()) {
            changes.push(Change {
                path: target.path.clone(),
                content,
            });
        }
    }
    Ok(changes)
}
pub fn apply(
    context: &crate::context::Context,
    changes: &[Change],
    selection: Option<&str>,
) -> Result<()> {
    if changes.is_empty() && selection.is_none() {
        return Ok(());
    }
    let mut transaction = crate::fs::transaction::Transaction::new(context)?;
    for change in changes {
        crate::cancel::check()?;
        transaction.write(&context.root.join(&change.path), change.content.as_bytes())?;
    }
    if let Some(selection) = selection {
        crate::cancel::check()?;
        transaction.write(
            &context.root_config.join("profiles.dotfile"),
            selection.as_bytes(),
        )?;
    }
    crate::cancel::check()?;
    transaction.commit()?;
    Ok(())
}

pub fn print(changes: &[Change], dry: bool) {
    let count = changes.len();
    println!(
        "  {}",
        if dry {
            format!(
                "{count} {} would change",
                if count == 1 { "file" } else { "files" }
            )
        } else {
            format!(
                "regenerated {count} {}",
                if count == 1 { "file" } else { "files" }
            )
        }
    );
    for change in changes {
        println!("      {}", change.path);
    }
}
