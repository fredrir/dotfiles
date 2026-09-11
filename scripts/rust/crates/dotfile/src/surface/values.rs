use crate::config::{self, Configuration, PackageKind};
use crate::context::Context;
use std::collections::BTreeSet;
use std::fs;

pub fn lines(context: &Context, source: &str, arguments: &[String]) -> Vec<String> {
    // A partial config must not print diagnostics into the command being completed.
    values(context, source, arguments)
        .unwrap_or_default()
        .into_iter()
        .filter(|v| !v.is_empty())
        .map(|v| v.replace(['\n', '\r', '\t'], " "))
        .collect()
}
fn escaped(s: &str) -> String {
    s.replace(':', "\\:")
}
fn row(value: &str, description: &str) -> String {
    format!(
        "{}:{}",
        escaped(value),
        description.replace(['\n', '\r', '\t'], " ")
    )
}
fn all_configuration(context: &Context) -> Result<Configuration, String> {
    let groups = crate::artifacts::packages::package_groups(context)?
        .into_iter()
        .filter(|g| context.root.join(g).is_dir())
        .collect::<Vec<_>>();
    Ok(Configuration {
        targets: config::load_targets(context)?,
        packages: config::collect_packages(context, &groups)?,
        groups,
        active_override_dirs: Vec::new(),
        overrides: Default::default(),
    })
}
fn values(context: &Context, source: &str, arguments: &[String]) -> Result<Vec<String>, String> {
    match source {
        "profiles" => {
            let relevant = config::profiles::relevant(context)?
                .into_iter()
                .collect::<BTreeSet<_>>();
            Ok(context
                .profiles()?
                .into_iter()
                .map(|p| {
                    if relevant.contains(&p) {
                        row(&p, "runs on this machine")
                    } else {
                        escaped(&p)
                    }
                })
                .collect())
        }
        "override-groups" => Ok(crate::artifacts::packages::package_groups(context)?
            .into_iter()
            .filter(|g| context.root.join(g).join("overrides").is_dir())
            .collect()),
        "override-names" => {
            let mut names = Vec::new();
            if let Some(group) = arguments.first() {
                config::validate_relative(group)?;
                names.extend(
                    config::sorted_directories(&context.root.join(group).join("overrides"))?
                        .into_iter()
                        .filter_map(|p| p.file_name().map(|n| escaped(&n.to_string_lossy()))),
                );
            }
            names.push(row("none", "link the group without an override"));
            Ok(names)
        }
        "packages" | "tracked" => Ok(all_configuration(context)?
            .packages
            .into_iter()
            .map(|p| {
                if source == "tracked" {
                    escaped(&p.name)
                } else {
                    row(&p.package, &p.name)
                }
            })
            .collect()),
        "recipients" => Ok(crate::secret::recipients::load(context)?
            .keys()
            .map(|k| escaped(k))
            .collect()),
        "secrets" => {
            let configuration = all_configuration(context)?;
            let mut rows = vec![row("vars", "the shared variables file")];
            for e in crate::secret::vault::plan(context, &configuration)? {
                rows.push(row(
                    &e.source
                        .strip_prefix(&context.root)
                        .unwrap_or(&e.source)
                        .to_string_lossy(),
                    &e.destination.to_string_lossy().replacen(
                        &context.home.to_string_lossy().to_string(),
                        "~",
                        1,
                    ),
                ));
            }
            Ok(rows)
        }
        "system-files" => {
            let configuration = all_configuration(context)?;
            let mut rows = Vec::new();
            for p in configuration
                .packages
                .iter()
                .filter(|p| p.kind == PackageKind::System)
            {
                for e in crate::secret::vault::package_entries(context, &configuration, p, true)? {
                    rows.push(row(
                        &e.destination.to_string_lossy(),
                        &e.source
                            .strip_prefix(&context.root)
                            .unwrap_or(&e.source)
                            .to_string_lossy(),
                    ));
                }
            }
            Ok(rows)
        }
        "theme-profiles" => {
            let mut names = fs::read_dir(context.root.join("theme/profiles"))
                .map_err(|e| e.to_string())?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "toml"))
                .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
                .collect::<Vec<_>>();
            names.sort();
            Ok(names)
        }
        "theme-scopes" => crate::theme::scopes(context),
        "dev-packages" => Ok(crate::dev::package_names()?.into_iter().collect()),
        "dev-languages" => Ok(crate::dev::language_names()),
        "hosts" => Ok(
            sysinfo::inventory::load_hosts_from(&context.inventory().hosts_file())?
                .into_iter()
                .map(|host| row(&host.name, &host.role))
                .collect(),
        ),
        _ => Ok(Vec::new()),
    }
}
