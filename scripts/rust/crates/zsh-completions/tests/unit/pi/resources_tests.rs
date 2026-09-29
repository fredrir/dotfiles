use super::*;
use testkit::tree;

fn pi_in(root: &Path) -> Pi {
    Pi {
        binary: None,
        package: Some(root.join("coding-agent")),
        agent_dir: root.join("agent"),
    }
}

#[test]
fn project_settings_override_global_ones_and_packages_add_up() {
    let root = tree(&[
        "agent/settings.json={\"defaultProvider\":\"google\",\"defaultModel\":\"g\",\"packages\":[\"npm:a\",{\"source\":\"git:b\"}]}",
        "work/.pi/settings.json={\"defaultModel\":\"p\",\"packages\":[\"./local\"]}",
    ]);
    let ctx = Context::testing(root.path(), &root.path().join("work"), &[]);
    let settings = Settings::load(&ctx, &pi_in(root.path()));
    assert_eq!(settings.default_provider.as_deref(), Some("google"));
    assert_eq!(settings.default_model.as_deref(), Some("p"));
    let sources: Vec<(&str, bool)> = settings
        .packages
        .iter()
        .map(|package| (package.source.as_str(), package.project))
        .collect();
    assert_eq!(
        sources,
        [("./local", true), ("npm:a", false), ("git:b", false)]
    );
}

#[test]
fn themes_come_from_pi_the_user_the_project_and_packages() {
    let root = tree(&[
        "coding-agent/dist/modes/interactive/theme/dark.json={\"name\":\"dark\"}",
        "coding-agent/dist/modes/interactive/theme/theme-schema.json={}",
        "agent/themes/nord.json={\"name\":\"Nord\"}",
        "agent/themes/plain.jsonc=// comments\n{}",
        "work/.pi/themes/dark.json={\"name\":\"dark\"}",
        "agent/npm/node_modules/pi-themes/package.json={\"pi\":{\"themes\":[\"./themes/*.json\"]}}",
        "agent/npm/node_modules/pi-themes/themes/solar.json={\"name\":\"solar\"}",
        "agent/npm/node_modules/pi-plain/themes/mono.json={}",
    ]);
    let ctx = Context::testing(root.path(), &root.path().join("work"), &[]);
    let pi = pi_in(root.path());
    let settings = Settings {
        packages: vec![
            PackageSource {
                source: "npm:pi-themes@1.0.0".into(),
                project: false,
            },
            PackageSource {
                source: "npm:pi-plain".into(),
                project: false,
            },
        ],
        ..Settings::default()
    };
    let themes: Vec<(String, String)> = themes(&ctx, &pi, &settings)
        .into_iter()
        .map(|theme| (theme.name, theme.origin))
        .collect();
    assert_eq!(
        themes,
        [
            ("dark".into(), "built-in".into()),
            ("Nord".into(), "user".into()),
            ("plain".into(), "user".into()),
            ("solar".into(), "npm:pi-themes@1.0.0".into()),
            ("mono".into(), "npm:pi-plain".into()),
        ]
    );
}

#[test]
fn package_directories_follow_their_source() {
    let root = tree(&[]);
    let ctx = Context::testing(root.path(), &root.path().join("work"), &[]);
    let pi = pi_in(root.path());
    let dir = |source: &str, project| {
        package_dir(
            &ctx,
            &pi,
            &PackageSource {
                source: source.into(),
                project,
            },
        )
    };
    assert_eq!(
        dir("npm:@a/b@2", false),
        Some(root.path().join("agent/npm/node_modules/@a/b"))
    );
    assert_eq!(
        dir("npm:c", true),
        Some(root.path().join("work/.pi/npm/node_modules/c"))
    );
    assert_eq!(
        dir("./local", false),
        Some(root.path().join("agent/./local"))
    );
    assert_eq!(dir("git:github.com/x/y", false), None);
}

#[test]
fn tool_names_are_read_from_pi_s_declaration() {
    let source = "import x;\nexport const allToolNames = new Set([\n    \"read\",\n    'bash',\n    \"edit\",\n]);\n";
    assert_eq!(declared_tools(source), ["read", "bash", "edit"]);
    assert!(declared_tools("nothing here").is_empty());
}
