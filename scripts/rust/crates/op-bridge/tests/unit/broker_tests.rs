use super::*;
use std::cell::Cell;

const DEV: &str = "op://Dev/pi/credential";
const SECURE: &str = "op://Secure/bank/password";

fn policy() -> Policy {
    Policy::new(vec!["Dev".to_string()], vec!["Secure".to_string()]).unwrap()
}

fn broker() -> Broker {
    Broker::new(policy(), GRANT, BTreeSet::new())
}

fn secret() -> Result<Zeroizing<String>, String> {
    Ok(Zeroizing::new("s3cret".to_string()))
}

fn value(response: Response) -> String {
    match response {
        Response::Value(value) => value.to_string(),
        other => panic!("{other:?}"),
    }
}

fn never(_: &str) -> Result<(), String> {
    panic!("prompted")
}

#[test]
fn vaults_are_matched_whole_and_regardless_of_case() {
    let policy = policy();
    assert_eq!(policy.tier("op://dev/pi/credential"), Ok(Tier::Silent));
    assert_eq!(policy.tier("op://SECURE/bank/password"), Ok(Tier::Prompt));
    assert!(policy.tier("op://Development/pi/credential").is_err());
    assert!(policy.tier("op://Personal/bank/password").is_err());
}

#[test]
fn a_vault_cannot_be_both_silent_and_prompted() {
    assert!(Policy::new(vec!["Dev".to_string()], vec!["dev".to_string()]).is_err());
}

#[test]
fn a_silent_vault_never_prompts_and_is_fetched_once() {
    let fetches = Cell::new(0);
    let fetch = |_: &str| {
        fetches.set(fetches.get() + 1);
        secret()
    };
    let mut broker = broker();
    let start = Instant::now();
    for origin in [Origin::Peer, Origin::Local, Origin::Peer] {
        assert_eq!(value(broker.resolve(DEV, origin, start, never, fetch)), "s3cret");
    }
    let tomorrow = start + Duration::from_secs(24 * 60 * 60);
    assert_eq!(value(broker.resolve(DEV, Origin::Peer, tomorrow, never, fetch)), "s3cret");
    assert_eq!(fetches.get(), 1);
}

#[test]
fn forgetting_empties_memory_but_keeps_the_references_to_refill() {
    let mut broker = broker();
    broker.resolve(DEV, Origin::Peer, Instant::now(), never, |_| secret());
    broker.forget();
    assert!(!broker.cached(DEV, Instant::now()));
    assert_eq!(broker.missing(), [DEV]);
    broker.store(DEV, Zeroizing::new("s3cret".to_string()));
    assert!(broker.missing().is_empty());
}

#[test]
fn only_newly_used_silent_references_are_reported_for_saving() {
    let mut broker = broker();
    let now = Instant::now();
    broker.resolve(DEV, Origin::Peer, now, never, |_| secret());
    assert_eq!(broker.take_known(), Some(vec![DEV.to_string()]));
    broker.forget();
    broker.resolve(DEV, Origin::Peer, now, never, |_| secret());
    assert_eq!(broker.take_known(), None);
    broker.resolve(SECURE, Origin::Peer, now, |_| Ok(()), |_| secret());
    assert_eq!(broker.take_known(), None);
}

#[test]
fn the_prompt_names_the_reference_and_the_destination() {
    let prompted = Cell::new(String::new());
    let mut broker = broker();
    broker.resolve(
        SECURE,
        Origin::Peer,
        Instant::now(),
        |reason| {
            prompted.set(reason.to_string());
            Ok(())
        },
        |_| secret(),
    );
    assert_eq!(prompted.take(), "send op://Secure/bank/password to archie");
}

#[test]
fn a_prompted_reference_is_sent_without_asking_again_within_the_grant() {
    let prompts = Cell::new(0);
    let approve = |_: &str| {
        prompts.set(prompts.get() + 1);
        Ok(())
    };
    let mut broker = broker();
    let start = Instant::now();
    broker.resolve(SECURE, Origin::Peer, start, approve, |_| secret());
    let later = start + GRANT - Duration::from_secs(1);
    let refetch = |_: &str| Err("should not fetch".to_string());
    assert_eq!(value(broker.resolve(SECURE, Origin::Peer, later, approve, refetch)), "s3cret");
    broker.resolve(SECURE, Origin::Peer, start + GRANT, approve, |_| secret());
    assert_eq!(prompts.get(), 2);
}

#[test]
fn a_sleep_ends_a_grant_early() {
    let prompts = Cell::new(0);
    let approve = |_: &str| {
        prompts.set(prompts.get() + 1);
        Ok(())
    };
    let mut broker = broker();
    let now = Instant::now();
    broker.resolve(SECURE, Origin::Peer, now, approve, |_| secret());
    broker.forget();
    broker.resolve(SECURE, Origin::Peer, now, approve, |_| secret());
    assert_eq!(prompts.get(), 2);
    assert!(broker.missing().is_empty(), "prompted vaults are never refilled");
}

#[test]
fn local_callers_need_touch_id_for_prompted_vaults() {
    let mut broker = broker();
    let mut asked = false;
    let response = broker.resolve(
        SECURE,
        Origin::Local,
        Instant::now(),
        |_| {
            asked = true;
            Ok(())
        },
        |_| Ok(Zeroizing::new("value".to_string())),
    );
    assert!(asked);
    assert!(matches!(response, Response::Value(_)), "{response:?}");
}

#[test]
fn a_declined_prompt_is_denied_and_never_fetches() {
    let mut broker = broker();
    let response = broker.resolve(
        SECURE,
        Origin::Peer,
        Instant::now(),
        |_| Err("Touch ID: UserCanceled".to_string()),
        |_| panic!("fetched after a declined prompt"),
    );
    assert!(matches!(response, Response::Denied(_)), "{response:?}");
}

#[test]
fn a_failed_fetch_is_refused_so_the_caller_falls_back_and_nothing_is_kept() {
    let mut broker = broker();
    let now = Instant::now();
    let response = broker.resolve(DEV, Origin::Peer, now, never, |_| {
        Err("item not found".to_string())
    });
    assert!(matches!(response, Response::Refused(_)), "{response:?}");
    assert!(!broker.cached(DEV, now));
    assert!(broker.missing().is_empty());
}

#[test]
fn an_unlisted_vault_is_refused_before_any_prompt_or_fetch() {
    let mut broker = broker();
    let response = broker.resolve(
        "op://Personal/bank/password",
        Origin::Peer,
        Instant::now(),
        never,
        |_| panic!("fetched an unlisted vault"),
    );
    assert!(matches!(response, Response::Refused(_)), "{response:?}");
}
