//! Validation (syn) and formatting (prettyplease) – extension of the fork (ADR-0013).

use grauwolf_codegen::{FormatError, Scope};

#[test]
fn formats_valid_code() {
    let mut scope = Scope::new();
    scope.new_fn("answer").vis("pub").ret("u32").line("40+2");

    assert_eq!(
        scope.to_formatted_string().unwrap(),
        "pub fn answer() -> u32 {\n    40 + 2\n}\n"
    );
}

#[test]
fn rejects_code_that_does_not_parse() {
    let mut scope = Scope::new();
    scope.import("std::sync", "Arc");
    scope.raw("//! Inner doc after an item is invalid");
    scope.new_fn("broken").line("let x = ;");

    assert!(matches!(
        scope.to_formatted_string(),
        Err(FormatError::Syntax { .. })
    ));
}
