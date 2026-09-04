use super::*;

#[test]
fn stderr_reasons_win_over_the_fallback() {
    assert_eq!(reason(b"no host\n", "fallback"), "no host");
    assert_eq!(reason(b"", "fallback"), "fallback");
}
