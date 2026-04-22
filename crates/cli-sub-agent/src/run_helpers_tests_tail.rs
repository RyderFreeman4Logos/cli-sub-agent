use super::truncate_prompt;

#[test]
fn truncate_prompt_empty_string() {
    assert_eq!(truncate_prompt("", 10), "");
}
