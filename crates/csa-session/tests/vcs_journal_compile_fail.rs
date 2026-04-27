#[test]
fn vcs_backend_and_journal_are_separate_traits() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/jj_journal_not_vcs_backend.rs");
}
