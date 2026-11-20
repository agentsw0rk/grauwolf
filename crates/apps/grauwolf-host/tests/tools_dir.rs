//! Tool directory that is loaded at runtime (ADR-0074).

use std::path::PathBuf;

use grauwolf_host::tools_dir::ToolsDir;
use grauwolf_toolbuild::ToolBuilder;
use serde_json::json;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// Fresh tool directory in the temporary directory.
fn tools_dir(name: &str) -> grauwolf_test::TestDir {
    grauwolf_test::TestDir::new(name)
}

fn write_tool(dir: &std::path::Path, file: &str, name: &str, code: &str) {
    std::fs::write(
        dir.join(file),
        json!({
            "name": name, "description": "Testtool",
            "args": { "type": "object", "properties": { "text": { "type": "string" } } },
            "code": code
        })
        .to_string(),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: builds tool code for WASM with cargo"]
async fn loads_usable_tools_and_reports_the_others() {
    let dir = tools_dir("tools-dir");
    write_tool(
        &dir,
        "upper.json",
        "upper",
        "Ok(args[\"text\"].as_str().unwrap_or(\"\").to_uppercase())",
    );
    write_tool(&dir, "broken.json", "broken", "Ok(undefined_value)");
    write_tool(&dir, "taken.json", "add", "Ok(String::new())");
    std::fs::write(dir.join("malformed.json"), "{ not json").unwrap();
    std::fs::write(dir.join("notes.txt"), "is ignored").unwrap();
    let tools = ToolsDir::new(
        &dir,
        ToolBuilder::new(workspace().join("target/toolbuild-tests")),
        vec!["add".to_owned()],
    )
    .unwrap();

    let loaded = tools.load().await;

    let names: Vec<&str> = loaded.tools.iter().map(|tool| tool.name()).collect();
    assert_eq!(names, ["upper"]);
    assert_eq!(
        loaded.tools[0]
            .call_json(json!({ "text": "wolf" }))
            .await
            .unwrap(),
        "WOLF"
    );
    let skipped = loaded.skipped.join("\n");
    assert!(skipped.contains("broken.json"), "{skipped}");
    assert!(
        skipped.contains("taken.json") && skipped.contains("add"),
        "{skipped}"
    );
    assert!(skipped.contains("malformed.json"), "{skipped}");
    assert!(!skipped.contains("notes.txt"), "{skipped}");
}

#[tokio::test(flavor = "multi_thread")]
async fn treats_a_missing_directory_as_empty() {
    let dir = grauwolf_test::TestDir::new("no-tools");

    let loaded = ToolsDir::new(
        &dir,
        ToolBuilder::new(workspace().join("target/toolbuild-tests")),
        Vec::new(),
    )
    .unwrap()
    .load()
    .await;

    assert!(loaded.tools.is_empty());
    assert_eq!(loaded.skipped, Vec::<String>::new());
}
