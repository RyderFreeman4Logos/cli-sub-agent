use super::question;
use crate::cli::{Cli, Commands, DebateArgs};
use clap::Parser;

fn parse_debate_args(argv: &[&str]) -> DebateArgs {
    let cli = Cli::try_parse_from(argv).expect("debate CLI args should parse");
    match cli.command {
        Commands::Debate(args) => args,
        _ => panic!("expected debate subcommand"),
    }
}

#[test]
fn debate_question_file_supplies_question() {
    let temp = tempfile::tempdir().unwrap();
    let question_file = temp.path().join("motion.md");
    std::fs::write(&question_file, "question from file").unwrap();
    let question_file_arg = question_file.display().to_string();
    let mut args = parse_debate_args(&["csa", "debate", "--question-file", &question_file_arg]);
    let mut stdin = std::io::Cursor::new("stdin should not win");

    let (resolved, difficulty) =
        question::build_debate_question_from_reader(&mut args, false, &mut stdin)
            .expect("question file should build");

    assert_eq!(resolved, "question from file");
    assert_eq!(difficulty, None);
}

#[test]
fn debate_empty_question_fails_with_clear_error() {
    let mut args = parse_debate_args(&["csa", "debate"]);
    let mut stdin = std::io::Cursor::new("");

    let err = question::build_debate_question_from_reader(&mut args, true, &mut stdin)
        .expect_err("missing question on terminal stdin must fail");

    let message = err.to_string();
    assert!(message.contains("debate question is empty"), "{message}");
    assert!(
        message.contains("stdin is not available to the detached daemon"),
        "{message}"
    );
    assert!(message.contains("--question-file QUESTION.md"), "{message}");
}

#[test]
fn debate_question_precedence_positional_then_file_then_stdin() {
    let temp = tempfile::tempdir().unwrap();
    let question_file = temp.path().join("motion.md");
    std::fs::write(&question_file, "question from file").unwrap();
    let question_file_arg = question_file.display().to_string();

    let mut positional_args = parse_debate_args(&[
        "csa",
        "debate",
        "--question-file",
        &question_file_arg,
        "question from positional",
    ]);
    let mut stdin = std::io::Cursor::new("question from stdin");
    let (resolved, _) =
        question::build_debate_question_from_reader(&mut positional_args, false, &mut stdin)
            .expect("positional question should win");
    assert_eq!(resolved, "question from positional");

    let mut file_args =
        parse_debate_args(&["csa", "debate", "--question-file", &question_file_arg]);
    let mut stdin = std::io::Cursor::new("question from stdin");
    let (resolved, _) =
        question::build_debate_question_from_reader(&mut file_args, false, &mut stdin)
            .expect("question file should win over stdin");
    assert_eq!(resolved, "question from file");
}

#[test]
fn debate_omitted_question_drains_piped_stdin() {
    let mut args = parse_debate_args(&["csa", "debate"]);
    let mut stdin = std::io::Cursor::new("question from piped stdin");

    let (resolved, difficulty) =
        question::build_debate_question_from_reader(&mut args, false, &mut stdin)
            .expect("piped stdin should become the question");

    assert_eq!(resolved, "question from piped stdin");
    assert_eq!(difficulty, None);
}

#[test]
fn debate_cli_parses_question_file() {
    let args = parse_debate_args(&["csa", "debate", "--question-file", "motion.md"]);

    assert_eq!(
        args.question_file.as_deref(),
        Some(std::path::Path::new("motion.md"))
    );
}
