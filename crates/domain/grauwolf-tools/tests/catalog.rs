//! The tool catalog index (ADR-0124): format, entries by name and version, search.

use grauwolf_tools::catalog::{
    CatalogEntry, CatalogError, CatalogIndex, CatalogSignature, CatalogTool, compare_versions,
};
use serde_json::json;

fn entry(name: &str, version: &str, description: &str, tool: (&str, &str)) -> CatalogEntry {
    CatalogEntry {
        reference: format!("registry.example.com/acme/{name}@sha256:{}", "a".repeat(64)),
        name: name.into(),
        version: version.into(),
        description: Some(description.into()),
        tools: vec![CatalogTool {
            name: tool.0.into(),
            description: tool.1.into(),
            args: json!({ "type": "object" }),
            module: format!("{}.wasm", tool.0),
            digest: format!("sha256:{}", "b".repeat(64)),
            size: 1234,
            limits: Default::default(),
            capabilities: Vec::new(),
        }],
        created: Some("2026-11-16T09:30:00Z".into()),
        source: Some("https://github.com/acme/tools".into()),
        revision: Some("c".repeat(40)),
        license: Some("MIT".into()),
        authors: vec!["Ada".into()],
        abi: 1,
        built_with: Some("0.1.0".into()),
        rustc: None,
        signature: CatalogSignature::default(),
    }
}

fn index() -> CatalogIndex {
    let mut index = CatalogIndex::new("2026-11-16T10:00:00Z");
    index.upsert(entry(
        "math",
        "1.9.0",
        "Arithmetic",
        ("add", "Adds two numbers"),
    ));
    index.upsert(entry(
        "math",
        "1.10.0",
        "Arithmetic",
        ("add", "Adds two numbers"),
    ));
    index.upsert(entry(
        "text",
        "0.1.0",
        "Words",
        ("count_words", "Counts the words"),
    ));
    index
}

#[test]
fn the_index_round_trips_through_json_with_entries_keyed_by_name_and_version() {
    let index = index();
    let json = index.to_json();
    assert!(json.contains("\"math@1.10.0\""), "{json}");
    assert_eq!(CatalogIndex::from_json(&json).unwrap(), index);
}

#[test]
fn an_index_of_another_format_is_refused() {
    let json = index().to_json().replace("\"format\": 1", "\"format\": 2");
    assert_eq!(
        CatalogIndex::from_json(&json).unwrap_err(),
        CatalogError::UnsupportedFormat { found: 2 }
    );
    assert!(matches!(
        CatalogIndex::from_json("[]").unwrap_err(),
        CatalogError::Format(_)
    ));
}

#[test]
fn upsert_replaces_the_entry_of_the_same_version_and_reports_changes() {
    let mut index = index();
    assert!(!index.upsert(entry(
        "text",
        "0.1.0",
        "Words",
        ("count_words", "Counts the words")
    )));
    assert!(index.upsert(entry("text", "0.1.0", "Changed", ("count_words", "Counts"))));
    assert_eq!(index.entries.len(), 3);
    assert_eq!(
        index
            .find("text", Some("0.1.0"))
            .unwrap()
            .description
            .as_deref(),
        Some("Changed")
    );
}

#[test]
fn remove_drops_one_version() {
    let mut index = index();
    assert!(index.remove("math", "1.9.0").is_some());
    assert!(index.remove("math", "1.9.0").is_none());
    assert_eq!(index.versions("math"), ["1.10.0"]);
}

#[test]
fn list_orders_by_name_and_version_and_find_takes_the_newest_without_a_version() {
    let index = index();
    let keys: Vec<String> = index.list().iter().map(|entry| entry.key()).collect();
    assert_eq!(keys, ["math@1.9.0", "math@1.10.0", "text@0.1.0"]);
    assert_eq!(index.find("math", None).unwrap().version, "1.10.0");
    assert!(index.find("none", None).is_none());
}

#[test]
fn search_looks_at_names_descriptions_and_tools_ignoring_case() {
    let index = index();
    let found = |text: &str| -> Vec<String> {
        index.search(text).iter().map(|entry| entry.key()).collect()
    };
    assert_eq!(found("TEXT"), ["text@0.1.0"]);
    assert_eq!(found("arithmetic").len(), 2);
    assert_eq!(found("count_words"), ["text@0.1.0"]);
    assert_eq!(found("adds two"), ["math@1.9.0", "math@1.10.0"]);
    assert!(found("nothing like this").is_empty());
}

#[test]
fn versions_compare_by_their_numeric_parts() {
    use std::cmp::Ordering::*;
    assert_eq!(compare_versions("1.10.0", "1.9.2"), Greater);
    assert_eq!(compare_versions("v2", "1.0"), Greater);
    assert_eq!(compare_versions("1.0", "1.0.1"), Less);
    assert_eq!(compare_versions("1.0.0", "1.0.0"), Equal);
}
