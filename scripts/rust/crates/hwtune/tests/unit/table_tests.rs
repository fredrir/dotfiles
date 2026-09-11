use super::*;

#[test]
fn columns_are_padded_to_the_widest_cell() {
    let text = render(
        &["metric", "value"],
        &[
            vec!["tctl".into(), "75.0".into()],
            vec!["radiator".into(), "1048".into()],
        ],
    );
    assert_eq!(text, "metric    value\ntctl      75.0\nradiator  1048\n");
}
