use super::*;
use std::cell::Cell;

const REFERENCE: &str = "op://Dev/pi/credential";

fn broker() -> Broker {
    Broker::new(vec!["Dev".to_string()], GRANT)
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

#[test]
fn the_prompt_names_the_reference_and_the_peer() {
    let prompted = Cell::new(String::new());
    let mut broker = broker();
    broker.resolve(
        REFERENCE,
        "archie",
        Instant::now(),
        |reason| {
            prompted.set(reason.to_string());
            Ok(())
        },
        |_| secret(),
    );
    assert_eq!(prompted.take(), "send op://Dev/pi/credential to archie");
}

#[test]
fn an_approved_reference_is_sent_without_asking_again_within_the_grant() {
    let prompts = Cell::new(0);
    let mut broker = broker();
    let start = Instant::now();
    let approve = |_: &str| {
        prompts.set(prompts.get() + 1);
        Ok(())
    };
    assert_eq!(
        value(broker.resolve(REFERENCE, "archie", start, approve, |_| secret())),
        "s3cret"
    );
    let later = start + GRANT - Duration::from_secs(1);
    let refetch = |_: &str| Err("should not fetch".to_string());
    assert_eq!(
        value(broker.resolve(REFERENCE, "archie", later, approve, refetch)),
        "s3cret"
    );
    assert_eq!(prompts.get(), 1);
}

#[test]
fn the_grant_expires_thirty_minutes_after_approval() {
    let prompts = Cell::new(0);
    let mut broker = broker();
    let start = Instant::now();
    let approve = |_: &str| {
        prompts.set(prompts.get() + 1);
        Ok(())
    };
    broker.resolve(REFERENCE, "archie", start, approve, |_| secret());
    assert!(!broker.granted(REFERENCE, start + GRANT));
    broker.resolve(REFERENCE, "archie", start + GRANT, approve, |_| secret());
    assert_eq!(prompts.get(), 2);
}

#[test]
fn a_grant_covers_only_the_approved_reference() {
    let mut broker = broker();
    let now = Instant::now();
    broker.resolve(REFERENCE, "archie", now, |_| Ok(()), |_| secret());
    assert!(broker.granted(REFERENCE, now));
    assert!(!broker.granted("op://Dev/pi/other", now));
}

#[test]
fn a_declined_prompt_is_denied_and_never_fetches() {
    let mut broker = broker();
    let response = broker.resolve(
        REFERENCE,
        "archie",
        Instant::now(),
        |_| Err("Touch ID: UserCanceled".to_string()),
        |_| panic!("fetched after a declined prompt"),
    );
    assert!(matches!(response, Response::Denied(_)), "{response:?}");
    assert!(!broker.granted(REFERENCE, Instant::now()));
}

#[test]
fn a_failed_fetch_is_refused_so_the_peer_falls_back_and_is_not_granted() {
    let mut broker = broker();
    let now = Instant::now();
    let response = broker.resolve(
        REFERENCE,
        "archie",
        now,
        |_| Ok(()),
        |_| Err("item not found".to_string()),
    );
    assert!(matches!(response, Response::Refused(_)), "{response:?}");
    assert!(!broker.granted(REFERENCE, now));
}

#[test]
fn a_vault_outside_the_list_is_refused_before_any_prompt() {
    let mut broker = broker();
    let response = broker.resolve(
        "op://Personal/bank/password",
        "archie",
        Instant::now(),
        |_| panic!("prompted for a refused vault"),
        |_| panic!("fetched a refused vault"),
    );
    assert!(matches!(response, Response::Refused(_)), "{response:?}");
}
