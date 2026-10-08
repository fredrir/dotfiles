use super::*;
use testkit::tree;

#[test]
fn the_project_file_wins_over_the_user_file() {
    let root = tree(&[
        "home/.npmrc=registry=https://user.example/\n@acme:registry=https://acme.example\n",
        "project/package.json={}",
        "project/.npmrc=registry=https://project.example/",
        "project/src/",
    ]);
    let ctx = Context::testing(
        &root.path().join("home"),
        &root.path().join("project/src"),
        &[],
    );
    let npmrc = Npmrc::load(&ctx);
    assert_eq!(npmrc.registry_for("react"), "https://project.example");
    assert_eq!(npmrc.registry_for("@acme/ui"), "https://acme.example");
    assert_eq!(npmrc.registry_for("@other/ui"), "https://project.example");
}

#[test]
fn the_environment_wins_over_every_file() {
    let root = tree(&["home/.npmrc=registry=https://user.example"]);
    let home = root.path().join("home");
    let ctx = Context::testing(
        &home,
        &home,
        &[("npm_config_registry", "https://env.example")],
    );
    assert_eq!(Npmrc::load(&ctx).registry_for("x"), "https://env.example");
}

#[test]
fn without_configuration_the_public_registry_is_used() {
    let root = tree(&[]);
    let ctx = Context::testing(root.path(), root.path(), &[]);
    assert_eq!(Npmrc::load(&ctx).registry_for("x"), DEFAULT_REGISTRY);
}

#[test]
fn values_expand_environment_variables_and_skip_comments() {
    let root = tree(&["home/.npmrc=# comment\n; also\nprefix=${BASE}/npm\ncache = \"~/c\"\n"]);
    let home = root.path().join("home");
    let ctx = Context::testing(&home, &home, &[("BASE", "/opt")]);
    let npmrc = Npmrc::load(&ctx);
    assert_eq!(npmrc.get("prefix").as_deref(), Some("/opt/npm"));
    assert_eq!(npmrc.get("CACHE").as_deref(), Some("~/c"));
    assert_eq!(npmrc.get("# comment"), None);
}
