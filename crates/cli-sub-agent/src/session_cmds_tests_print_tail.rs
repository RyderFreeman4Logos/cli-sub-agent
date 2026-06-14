use super::*;

#[test]
fn print_content_with_tail_no_panic_on_empty() {
    print_content_with_tail("", None);
    print_content_with_tail("", Some(5));
}

#[test]
fn print_content_with_tail_no_panic_on_large_tail() {
    print_content_with_tail("line1\nline2\n", Some(100));
}
