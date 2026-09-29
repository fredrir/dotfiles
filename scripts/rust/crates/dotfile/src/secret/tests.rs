use super::{identity, recipients, variables};
use crate::context::Context;
use std::collections::BTreeMap;

fn context() -> (tempfile::TempDir, Context) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("repo");
    std::fs::create_dir_all(root.join("config")).unwrap();
    let home = temporary.path().join("home");
    let context = Context::new(
        root.clone(),
        home.clone(),
        root.join("config"),
        home.join(".config"),
    )
    .unwrap();
    (temporary, context)
}

#[test]
fn recipients_parse_validate_and_format_stably() {
    let (_temporary, context) = context();
    let path = context.root_config.join("keys.dotfile");
    assert!(recipients::load(&context).unwrap().is_empty());
    let key = format!("age1{}", "q".repeat(58));
    std::fs::write(&path, format!("recipients {{\n  alpha = {key}\n}}\n")).unwrap();
    let parsed = recipients::load(&context).unwrap();
    assert_eq!(parsed.get("alpha"), Some(&key));
    assert_eq!(
        recipients::document(&context, &parsed).unwrap(),
        format!("recipients {{\n  alpha = {key}\n}}\n")
    );
    assert_eq!(
        recipients::policy(&parsed),
        format!("creation_rules:\n  - age: {key}\n")
    );
    assert!(recipients::policy(&BTreeMap::new()).is_empty());
    for invalid in [
        "recipients {\n alpha = invalid\n}\n".to_string(),
        format!("recipients {{\n alpha = {key}\n alpha = {key}\n}}\n"),
        format!("recipients {{\n alpha = {key}\n"),
        format!("other {{\n alpha = {key}\n}}\n"),
    ] {
        std::fs::write(&path, invalid).unwrap();
        assert!(recipients::load(&context).is_err());
    }
}

#[test]
fn identities_parse_alongside_recipients_and_render_after_them() {
    let (_temporary, context) = context();
    let key = format!("age1{}", "q".repeat(58));
    std::fs::write(
        context.root_config.join("keys.dotfile"),
        format!(
            "recipients {{\n  alpha = {key}\n}}\n\nidentities {{\n  alpha = op://Secure/ALPHA KEY\n  beta  = op://Dev/BETA\n}}\n"
        ),
    )
    .unwrap();
    let identities = identity::load(&context).unwrap();
    assert_eq!(
        identities["alpha"],
        identity::Reference {
            vault: "Secure".into(),
            item: "ALPHA KEY".into()
        }
    );
    assert_eq!(
        identities["beta"].field("credential"),
        "op://Dev/BETA/credential"
    );
    let recipients = recipients::load(&context).unwrap();
    assert_eq!(
        recipients::document(&context, &recipients).unwrap(),
        format!(
            "recipients {{\n  alpha = {key}\n}}\n\nidentities {{\n  alpha = op://Secure/ALPHA KEY\n  beta  = op://Dev/BETA\n}}\n"
        )
    );
    for invalid in [
        "Secure/ALPHA",
        "op://Secure",
        "op:///ALPHA",
        "op://Secure/",
        "op://Secure/ALPHA/credential",
        "op://Secure/ $(boom)",
        "op://Secure/it's",
        "op://Sec\"ure/ALPHA",
    ] {
        assert_eq!(identity::Reference::parse(invalid), None, "{invalid}");
    }
}

#[test]
fn generated_identities_report_their_own_public_key() {
    let (secret, public) = identity::generate();
    assert_eq!(identity::public_key(&secret).unwrap(), public);
    assert!(identity::public_key(&format!("{}\n{}", *secret, *secret)).is_err());
    assert!(identity::public_key("not a key").is_err());
}

#[test]
fn variables_flatten_scalars_and_reject_lists_nulls() {
    let mapping =
        serde_json::json!({"a":{"b":{"c":"x"}},"port":22,"on":true,"off":false,"ratio":1.5});
    let mut values = BTreeMap::new();
    variables::flatten_variables(mapping.as_object().unwrap(), "", &mut values).unwrap();
    assert_eq!(values["a.b.c"], "x");
    assert_eq!(values["port"], "22");
    assert_eq!(values["on"], "true");
    assert_eq!(values["off"], "false");
    assert_eq!(values["ratio"], "1.5");
    for (mapping, message) in [
        (serde_json::json!({"hosts":["a","b"]}), "list"),
        (serde_json::json!({"host":null}), "no value"),
    ] {
        assert!(
            variables::flatten_variables(mapping.as_object().unwrap(), "", &mut BTreeMap::new())
                .unwrap_err()
                .contains(message)
        );
    }
}

#[test]
fn templates_preserve_literals_and_deduplicate_missing_names() {
    assert_eq!(
        variables::references("{{ a.b }} {{a.b}} {{  c  }}"),
        vec!["a.b", "c"]
    );
    let values = BTreeMap::from([("a".to_string(), "$1 literal".to_string())]);
    assert_eq!(
        variables::render_template("{{ a }} {{ b }} {{b}}", &values),
        ("$1 literal {{ b }} {{b}}".into(), vec!["b".into()])
    );
    let literal = "${HOME} and {single} and }}{{";
    assert_eq!(
        variables::render_template(literal, &values),
        (literal.into(), Vec::new())
    );
    assert_eq!(
        variables::references("{{ invalid {{ valid }}"),
        vec!["valid"]
    );
    assert!(variables::references("{{ -invalid }} {{.invalid}}").is_empty());
}

#[test]
fn failed_second_install_restores_originals_and_cleans_journal() {
    let (_temporary, context) = context();
    let first = context.root_config.join("first");
    let second = context.root_config.join("second");
    std::fs::write(&first, b"first original").unwrap();
    std::fs::write(&second, b"second original").unwrap();
    let result = recipients::commit_inner(
        &context,
        vec![
            (first.clone(), b"first changed".to_vec()),
            (second.clone(), b"second changed".to_vec()),
        ],
        |index| {
            if index == 1 {
                assert_eq!(std::fs::read(&first).unwrap(), b"first changed");
                Err("injected install failure".into())
            } else {
                Ok(())
            }
        },
    );
    assert!(result.unwrap_err().contains("injected"));
    assert_eq!(std::fs::read(first).unwrap(), b"first original");
    assert_eq!(std::fs::read(second).unwrap(), b"second original");
    assert!(!context.root_config.join("secret-transaction").exists());
}
