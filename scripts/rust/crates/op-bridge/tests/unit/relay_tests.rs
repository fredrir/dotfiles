use super::*;
use std::cell::Cell;

const DEV: &str = "op://Dev/pi/credential";
const SECURE: &str = "op://Secure/bank/password";

fn memory() -> Mutex<Memory> {
    let policy = Policy::new(vec!["Dev".to_string()], vec!["Secure".to_string()]).unwrap();
    Mutex::new(Memory::new(policy))
}

fn read(reference: &str) -> Request {
    Request::Read(reference.to_string())
}

fn secret(_: &Request) -> Result<Response, String> {
    Ok(Response::Value(Zeroizing::new("s3cret".to_string())))
}

fn unreachable(_: &Request) -> Result<Response, String> {
    Err("macie unreachable: connection refused".to_string())
}

fn never(_: &Request) -> Result<Response, String> {
    panic!("asked macie")
}

fn value(response: Response) -> String {
    match response {
        Response::Value(value) => value.to_string(),
        Response::Refused(reason) | Response::Denied(reason) => panic!("{reason}"),
        _ => panic!("not a value"),
    }
}

#[test]
fn a_dev_value_is_kept_and_sent_without_asking_macie_again() {
    let memory = memory();
    assert_eq!(value(answer(&memory, &read(DEV), secret).unwrap()), "s3cret");
    for _ in 0..3 {
        assert_eq!(value(answer(&memory, &read(DEV), never).unwrap()), "s3cret");
    }
}

#[test]
fn a_kept_value_outlives_macie() {
    let memory = memory();
    answer(&memory, &read(DEV), secret).unwrap();
    assert_eq!(
        value(answer(&memory, &read(DEV), unreachable).unwrap()),
        "s3cret"
    );
}

#[test]
fn a_prompted_value_is_never_kept() {
    let memory = memory();
    let asked = Cell::new(0);
    for _ in 0..2 {
        let response = answer(&memory, &read(SECURE), |request| {
            asked.set(asked.get() + 1);
            secret(request)
        });
        assert_eq!(value(response.unwrap()), "s3cret");
    }
    assert_eq!(asked.get(), 2);
}

#[test]
fn an_unlisted_vault_is_left_to_macie_and_never_kept() {
    let memory = memory();
    let reference = "op://Personal/bank/password";
    answer(&memory, &read(reference), secret).unwrap();
    let response = answer(&memory, &read(reference), unreachable).unwrap();
    assert!(matches!(response, Response::Refused(_)));
}

#[test]
fn macie_unreachable_on_a_miss_is_refused_so_the_caller_falls_back() {
    let response = answer(&memory(), &read(DEV), unreachable).unwrap();
    match response {
        Response::Refused(reason) => assert!(reason.contains("macie unreachable"), "{reason}"),
        _ => panic!("not refused"),
    }
}

#[test]
fn a_denied_read_is_passed_on_and_not_kept() {
    let memory = memory();
    let denied = |_: &Request| Ok(Response::Denied("dismissed".to_string()));
    let response = answer(&memory, &read(DEV), denied).unwrap();
    assert!(matches!(response, Response::Denied(_)));
    assert!(matches!(
        answer(&memory, &read(DEV), unreachable).unwrap(),
        Response::Refused(_)
    ));
}

#[test]
fn a_reload_clears_archie_and_reaches_macie() {
    let memory = memory();
    answer(&memory, &read(DEV), secret).unwrap();
    let reloaded = |request: &Request| {
        assert!(matches!(request, Request::Reload));
        Ok(Response::Reloaded {
            refilled: 1,
            known: 1,
        })
    };
    let response = answer(&memory, &Request::Reload, reloaded).unwrap();
    assert!(matches!(
        response,
        Response::Reloaded {
            refilled: 1,
            known: 1
        }
    ));
    assert!(matches!(
        answer(&memory, &read(DEV), unreachable).unwrap(),
        Response::Refused(_)
    ));
}

#[test]
fn a_reload_without_macie_clears_archie_and_says_so() {
    let memory = memory();
    answer(&memory, &read(DEV), secret).unwrap();
    match answer(&memory, &Request::Reload, unreachable).unwrap() {
        Response::Refused(reason) => assert!(reason.contains("cleared archie only"), "{reason}"),
        _ => panic!("not refused"),
    }
    assert!(memory.lock().unwrap().kept.is_empty());
}

#[test]
fn macies_forget_clears_archie_without_asking_macie() {
    let memory = memory();
    answer(&memory, &read(DEV), secret).unwrap();
    let response = answer(&memory, &Request::Forget, never).unwrap();
    assert!(matches!(response, Response::Forgotten));
    assert!(memory.lock().unwrap().kept.is_empty());
}

#[test]
fn a_value_fetched_across_a_forget_is_not_kept() {
    let memory = memory();
    let straddling = |request: &Request| {
        memory.lock().unwrap().forget();
        secret(request)
    };
    assert_eq!(
        value(answer(&memory, &read(DEV), straddling).unwrap()),
        "s3cret"
    );
    assert!(memory.lock().unwrap().kept.is_empty());
}
