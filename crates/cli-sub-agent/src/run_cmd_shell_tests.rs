use super::{
    command_contains_forbidden_no_verify_commit, command_contains_git_commit,
    detect_git_commit_commands, detect_no_verify_commit_commands, tokenize_shell_tokens,
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

#[test]
fn command_contains_git_commit_detects_plain_and_global_option_forms() {
    assert!(command_contains_git_commit("git commit -m fix"));
    assert!(command_contains_git_commit(
        "git -C /tmp/repo commit --message fix"
    ));
    assert!(command_contains_git_commit(
        "env FOO=1 sudo nice git commit -m fix"
    ));
}

#[test]
fn command_contains_git_commit_detects_shell_payload() {
    assert!(command_contains_git_commit(
        "bash -lc \"git add src/lib.rs && git commit -m fix\""
    ));
}

#[test]
fn command_contains_git_commit_rejects_non_commit_git_commands() {
    assert!(!command_contains_git_commit("git push origin HEAD"));
    assert!(!command_contains_git_commit("git commit-tree HEAD^{tree}"));
    assert!(!command_contains_git_commit("echo git commit -m fix"));
}

#[test]
fn detect_git_commit_commands_dedupes_matches() {
    let commands = vec![
        "git status".to_string(),
        "git commit -m fix".to_string(),
        "git commit -m fix".to_string(),
        "bash -lc \"git commit -m nested\"".to_string(),
    ];

    let matches = detect_git_commit_commands(&commands);

    assert_eq!(
        matches,
        vec![
            "git commit -m fix".to_string(),
            "bash -lc \"git commit -m nested\"".to_string()
        ]
    );
}
