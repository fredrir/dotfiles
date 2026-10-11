use super::*;

#[test]
fn each_verify_difference_is_a_warning() {
    let differences = [
        Difference {
            table: "functions".to_string(),
            name: "lsl".to_string(),
            bundle: Some("\tll \"$@\"".to_string()),
            sources: Some("\tls -l \"$@\"".to_string()),
        },
        Difference {
            table: "aliases".to_string(),
            name: "extra".to_string(),
            bundle: None,
            sources: Some("print extra".to_string()),
        },
    ];
    let hint = Some("zsh-build verify -v".to_string());
    assert_eq!(
        verify_warnings(&differences),
        [
            (
                "zsh-build verify: functions lsl: bundle ll \"$@\", sources ls -l \"$@\""
                    .to_string(),
                hint.clone()
            ),
            (
                "zsh-build verify: aliases extra: bundle (none), sources print extra".to_string(),
                hint
            ),
        ]
    );
    assert!(verify_warnings(&[]).is_empty());
}
