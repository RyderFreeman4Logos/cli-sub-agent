use super::{
    command_contains_forbidden_no_verify_commit, detect_no_verify_commit_commands,
    tokenize_shell_tokens,
};

#[test]
fn detect_no_verify_commit_commands_ignores_following_commands_after_newline() {
    assert!(!command_contains_forbidden_no_verify_commit(
        "git commit -m msg\necho -n ok"
    ));
}

#[test]
fn tokenize_shell_tokens_treats_newline_as_command_separator() {
    let tokens = tokenize_shell_tokens("git commit -m msg\necho -n ok");
    assert_eq!(
        tokens,
        ["git", "commit", "-m", "msg", ";", "echo", "-n", "ok"]
    );
}

#[test]
fn tokenize_shell_tokens_drops_escaped_newlines() {
    let tokens = tokenize_shell_tokens("git commit -m msg\\\necho ok");
    assert_eq!(tokens, ["git", "commit", "-m", "msgecho", "ok"]);
}

#[test]
fn detect_no_verify_commit_commands_ignores_multiline_scripts() {
    let commands = vec!["git commit -m msg\necho -n ok".to_string()];
    assert!(detect_no_verify_commit_commands(&commands).is_empty());
}

#[test]
fn command_contains_forbidden_no_verify_commit_detects_prefixed_commit_in_shell_payload() {
    assert!(command_contains_forbidden_no_verify_commit(
        "bash -lc \"sudo git commit -n -m unsafe\""
    ));
    assert!(command_contains_forbidden_no_verify_commit(
        "bash -lc \"env -i git commit --no-verify -m unsafe\""
    ));
}
