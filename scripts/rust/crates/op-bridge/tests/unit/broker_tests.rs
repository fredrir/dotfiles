use super::*;
use std::cell::{Cell, RefCell};

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

fn never() -> Result<(), String> {
    panic!("signed out")
}

fn signed_out() -> Result<(), String> {
    Ok(())
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
fn a_silent_vault_keeps_the_session_and_is_fetched_once() {
    let fetches = Cell::new(0);
    let fetch = |_: &str| {
        fetches.set(fetches.get() + 1);
        secret()
    };
    let mut broker = broker();
    let start = Instant::now();
    for _ in 0..3 {
        assert_eq!(value(broker.resolve(DEV, start, never, fetch)), "s3cret");
    }
    let tomorrow = start + Duration::from_secs(24 * 60 * 60);
    assert_eq!(value(broker.resolve(DEV, tomorrow, never, fetch)), "s3cret");
    assert_eq!(fetches.get(), 1);
}

#[test]
fn forgetting_empties_memory_but_keeps_the_references_to_refill() {
    let mut broker = broker();
    broker.resolve(DEV, Instant::now(), never, |_| secret());
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
    broker.resolve(DEV, now, never, |_| secret());
    assert_eq!(broker.take_known(), Some(vec![DEV.to_string()]));
    broker.forget();
    broker.resolve(DEV, now, never, |_| secret());
    assert_eq!(broker.take_known(), None);
    broker.resolve(SECURE, now, signed_out, |_| secret());
    assert_eq!(broker.take_known(), None);
}

#[test]
fn a_refill_is_due_once_after_startup_and_after_each_sleep() {
    let mut broker = broker();
    assert!(broker.take_stale());
    assert!(!broker.take_stale());
    broker.forget();
    assert!(broker.take_stale());
    assert!(!broker.take_stale());
}

#[test]
fn a_prompted_reference_signs_out_first_so_1password_prompts() {
    let steps = RefCell::new(Vec::new());
    let mut broker = broker();
    broker.resolve(
        SECURE,
        Instant::now(),
        || {
            steps.borrow_mut().push("sign out");
            Ok(())
        },
        |_| {
            steps.borrow_mut().push("fetch");
            secret()
        },
    );
    assert_eq!(steps.take(), ["sign out", "fetch"]);
}

#[test]
fn a_prompted_reference_is_sent_without_asking_again_within_the_grant() {
    let prompts = Cell::new(0);
    let sign_out = || {
        prompts.set(prompts.get() + 1);
        Ok(())
    };
    let mut broker = broker();
    let start = Instant::now();
    broker.resolve(SECURE, start, sign_out, |_| secret());
    let later = start + GRANT - Duration::from_secs(1);
    let refetch = |_: &str| Err("should not fetch".to_string());
    assert_eq!(value(broker.resolve(SECURE, later, sign_out, refetch)), "s3cret");
    broker.resolve(SECURE, start + GRANT, sign_out, |_| secret());
    assert_eq!(prompts.get(), 2);
}

#[test]
fn a_sleep_ends_a_grant_early() {
    let prompts = Cell::new(0);
    let sign_out = || {
        prompts.set(prompts.get() + 1);
        Ok(())
    };
    let mut broker = broker();
    let now = Instant::now();
    broker.resolve(SECURE, now, sign_out, |_| secret());
    broker.forget();
    broker.resolve(SECURE, now, sign_out, |_| secret());
    assert_eq!(prompts.get(), 2);
    assert!(broker.missing().is_empty(), "prompted vaults are never refilled");
}

#[test]
fn a_failed_sign_out_is_denied_and_never_fetches() {
    let mut broker = broker();
    let response = broker.resolve(
        SECURE,
        Instant::now(),
        || Err("op signout exited with 1".to_string()),
        |_| panic!("fetched without a fresh 1Password prompt"),
    );
    assert!(matches!(response, Response::Denied(_)), "{response:?}");
}

#[test]
fn a_declined_prompt_is_denied_so_the_caller_does_not_ask_again() {
    let mut broker = broker();
    let now = Instant::now();
    let response = broker.resolve(SECURE, now, signed_out, |_| {
        Err("authorization prompt dismissed".to_string())
    });
    assert!(matches!(response, Response::Denied(_)), "{response:?}");
    assert!(!broker.cached(SECURE, now));
}

#[test]
fn a_failed_fetch_is_refused_so_the_caller_falls_back_and_nothing_is_kept() {
    let mut broker = broker();
    let now = Instant::now();
    let response = broker.resolve(DEV, now, never, |_| Err("item not found".to_string()));
    assert!(matches!(response, Response::Refused(_)), "{response:?}");
    assert!(!broker.cached(DEV, now));
    assert!(broker.missing().is_empty());
}

#[test]
fn an_unlisted_vault_is_refused_before_any_prompt_or_fetch() {
    let mut broker = broker();
    let response = broker.resolve(
        "op://Personal/bank/password",
        Instant::now(),
        never,
        |_| panic!("fetched an unlisted vault"),
    );
    assert!(matches!(response, Response::Refused(_)), "{response:?}");
}
