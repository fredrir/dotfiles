use super::*;

#[test]
fn relative_ages_are_compact() {
    let now = SystemTime::now();
    assert_eq!(relative_age(now), "now");
    assert_eq!(relative_age(now - Duration::from_secs(120)), "2m");
    assert!(timestamp_millis(now) > timestamp_millis(UNIX_EPOCH));
}

#[test]
fn stable_keys_include_the_source_host_and_agent() {
    assert_eq!(key(Host::Macie, Agent::Codex, "id"), "macie:codex:id");
    assert_ne!(
        key(Host::Macie, Agent::Codex, "id"),
        key(Host::Archie, Agent::Codex, "id")
    );
}

#[test]
fn remote_keys_preserve_ids_containing_colons() {
    let (agent, id) = remote_key(Host::Archie, "archie:claude:id:with:colons").unwrap();
    assert_eq!(agent, Agent::Claude);
    assert_eq!(id, "id:with:colons");
}

#[test]
fn remote_warnings_name_the_owning_host() {
    assert_eq!(
        remote_warning(Host::Archie, "codex session store: read failure".to_owned()),
        "archie: codex session store: read failure"
    );
}

#[test]
fn plain_listing_omits_disabled_diagnostic_rows() {
    let valid = SessionEntry {
        key: "macie:codex:valid".into(),
        id: "valid".into(),
        agent: Agent::Codex,
        origin: Origin::Local,
        host: Some("macie".into()),
        project: "dotfiles".into(),
        workspace: "~/dotfiles".into(),
        title: "A valid session".into(),
        updated: "now".into(),
        current_project: true,
        favorite: false,
        disabled_reason: None,
        warning: None,
        sort_timestamp: 1,
    };
    let mut diagnostic = valid.clone();
    diagnostic.key = "macie:diagnostic:codex:0".into();
    diagnostic.id = "diagnostic".into();
    diagnostic.disabled_reason = Some("malformed transcript".into());

    let listed = [valid, diagnostic]
        .into_iter()
        .filter(listable)
        .collect::<Vec<_>>();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "valid");
}
