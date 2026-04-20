use super::*;
use crate::parser::parse_skill;

#[test]
fn test_compile_step_with_blank_condition_hint_treated_as_none() {
    let cases = [
        ("blank-condition", "Condition:\nRun the final gate.\n"),
        (
            "whitespace-condition",
            "Condition:    \nRun the final gate.\n",
        ),
        (
            "nonblank-condition",
            "Condition: foo\nRun the final gate.\n",
        ),
    ];

    for (name, body) in cases {
        let input = format!("---\nname = \"{name}\"\n---\n## Review Gate\nTool: bash\n{body}");
        let doc = parse_skill(&input).unwrap();
        let plan = compile(&doc).unwrap();
        let step = &plan.steps[0];

        let expected = match name {
            "nonblank-condition" => Some("foo"),
            _ => None,
        };
        assert_eq!(step.condition.as_deref(), expected, "case={name}");
        assert_eq!(step.prompt, "Run the final gate.");
    }
}
