use super::tests::contains_relative_redirect;

#[test]
fn contains_relative_redirect_catches_multiple_redirects_per_line() {
    assert!(contains_relative_redirect(
        "printf 'a' > /abs/path; printf 'b' > rel/path"
    ));
}

#[test]
fn contains_relative_redirect_catches_quoted_relative_target() {
    assert!(contains_relative_redirect("printf 'a' > \"tracked.txt\""));
    assert!(contains_relative_redirect(
        "printf 'a' >> 'output/details.md'"
    ));
}

#[test]
fn contains_relative_redirect_allows_quoted_absolute_target() {
    assert!(!contains_relative_redirect("printf 'a' > \"/abs/path\""));
    assert!(!contains_relative_redirect("printf 'a' > \"{TMPDIR}/x\""));
}

#[test]
fn contains_relative_redirect_allows_interpolated_target() {
    assert!(!contains_relative_redirect("printf 'a' >> {tracked}"));
    assert!(!contains_relative_redirect("printf 'a' > ${TMPDIR}/x"));
}
