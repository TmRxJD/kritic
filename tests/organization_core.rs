use kritic::organization::{OrganizationPolicy, audit};

#[test]
fn oversized_file_without_functions_is_not_invisible_to_quality_review() {
    let source = "export const value = 1;\n".repeat(1001);
    let files = [("src/constants.ts".into(), source)];
    let result = audit(&files, &OrganizationPolicy::default());
    assert!(!result.approval);
    assert_eq!(result.files[0].physical_lines, 1001);
    assert_eq!(result.files[0].nonblank_lines, 1001);
    assert!(!result.violations.is_empty());
}
