//! Tests of the tool `{{tool}}`: its code body as a plain function.

use serde_json::{Value, json};

fn {{tool}}(args: Value) -> Result<String, String> {
    include!(concat!(env!("OUT_DIR"), "/{{tool}}.rs"))
}

#[test]
fn the_example_upper_cases_its_input() {
    assert_eq!({{tool}}(json!({ "input": "ada" })).unwrap(), "ADA");
}

#[test]
fn a_missing_input_is_an_error_for_the_model() {
    assert_eq!({{tool}}(json!({})).unwrap_err(), "input is missing");
}
