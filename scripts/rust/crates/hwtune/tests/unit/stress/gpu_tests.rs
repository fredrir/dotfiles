use super::*;

#[test]
fn tools_loop_forever_until_killed() {
    assert_eq!(
        command(GpuTool::Vkmark),
        ("vkmark".to_string(), vec!["--run-forever".to_string()])
    );
    assert_eq!(command(GpuTool::Glmark2).0, "glmark2");
}
