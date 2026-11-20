//! Factory for tools that agents build themselves (ADR-0075), with a real build and sandbox.

use std::path::PathBuf;

use grauwolf_host::self_tools::BuildingToolFactory;
use grauwolf_toolbuild::ToolBuilder;
use grauwolf_tools::{NewTool, ToolFactory};
use serde_json::json;

fn builder() -> ToolBuilder {
    ToolBuilder::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/toolbuild-tests"),
    )
}

fn new_tool(name: &str, code: &str) -> NewTool {
    NewTool {
        name: name.into(),
        description: "Converts text to upper case".into(),
        args: json!({ "type": "object", "properties": { "text": { "type": "string" } } }),
        code: code.into(),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: builds tool code for WASM with cargo"]
async fn builds_working_code_and_reports_compile_errors_as_text() {
    let factory = BuildingToolFactory::new(builder()).unwrap();

    let upper = factory
        .create(new_tool(
            "upper_self",
            "Ok(args[\"text\"].as_str().unwrap_or(\"\").to_uppercase())",
        ))
        .await
        .unwrap();
    let broken = factory
        .create(new_tool(
            "broken_self",
            "let x: u32 = \"text\";\nOk(x.to_string())",
        ))
        .await
        .err()
        .unwrap();

    assert_eq!(
        upper.call_json(json!({ "text": "wolf" })).await.unwrap(),
        "WOLF"
    );
    assert!(broken.contains("does not compile"), "{broken}");
    assert!(broken.contains("line 1, column 14"), "{broken}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: builds tool code for WASM with cargo"]
async fn saves_built_tools_so_the_tools_directory_offers_them_later() {
    let dir = grauwolf_test::TestDir::new("self-save");
    let factory = BuildingToolFactory::new(builder())
        .unwrap()
        .with_save_dir(&dir);

    factory
        .create(new_tool(
            "upper_saved",
            "Ok(args[\"text\"].as_str().unwrap_or(\"\").to_uppercase())",
        ))
        .await
        .unwrap();
    let loaded = grauwolf_host::tools_dir::ToolsDir::new(&dir, builder(), Vec::new())
        .unwrap()
        .load()
        .await;

    assert!(dir.join("upper_saved.rs").exists());
    assert_eq!(loaded.skipped, Vec::<String>::new());
    assert_eq!(loaded.tools.len(), 1);
    let result = loaded.tools[0].call_json(json!({ "text": "pack" })).await;
    assert_eq!(result.unwrap(), "PACK");
}
