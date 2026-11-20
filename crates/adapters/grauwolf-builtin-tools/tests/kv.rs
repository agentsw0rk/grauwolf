//! Key-value store tools (ADR-0098).

use std::path::PathBuf;
use std::sync::Arc;

use grauwolf_builtin_tools::{KV_TOOLS, KvTools, MAX_KEY_BYTES, MAX_VALUE_BYTES};
use grauwolf_tools::DynTool;
use serde_json::json;

fn file(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/builtin-tools-tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("kv.json")
}

fn tools(path: &PathBuf) -> Vec<Arc<dyn DynTool>> {
    KvTools::new(path).tools(KV_TOOLS.iter().copied()).unwrap()
}

async fn call(tools: &[Arc<dyn DynTool>], name: &str, args: serde_json::Value) -> String {
    let tool = tools.iter().find(|tool| tool.name() == name).unwrap();
    match tool.call_json(args).await {
        Ok(text) => text,
        Err(error) => error.to_string(),
    }
}

#[tokio::test]
async fn stores_retrieves_and_lists_values_in_one_json_file() {
    let path = file("kv-basic");
    let tools = tools(&path);

    assert_eq!(call(&tools, "list_keys", json!({})).await, "No keys stored");
    assert_eq!(
        call(&tools, "store_value", json!({ "key": "b", "value": "two" })).await,
        "Stored key: b"
    );
    call(&tools, "store_value", json!({ "key": "a", "value": "one" })).await;
    call(
        &tools,
        "store_value",
        json!({ "key": "b", "value": "second" }),
    )
    .await;

    assert_eq!(
        call(&tools, "retrieve_value", json!({ "key": "b" })).await,
        "second"
    );
    assert_eq!(
        call(&tools, "retrieve_value", json!({ "key": "zzz" })).await,
        "Key not found: zzz"
    );
    assert_eq!(call(&tools, "list_keys", json!({})).await, "a\nb");
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(stored, json!({ "a": "one", "b": "second" }));
    assert!(!path.with_extension("json.tmp").exists());
}

#[tokio::test]
async fn values_survive_new_tool_instances() {
    let path = file("kv-persist");
    call(
        &tools(&path),
        "store_value",
        json!({ "key": "k", "value": "v" }),
    )
    .await;

    assert_eq!(
        call(&tools(&path), "retrieve_value", json!({ "key": "k" })).await,
        "v"
    );
}

#[tokio::test]
async fn refuses_keys_and_values_over_the_limits() {
    let tools = tools(&file("kv-limits"));

    let long_key = call(
        &tools,
        "store_value",
        json!({ "key": "k".repeat(MAX_KEY_BYTES + 1), "value": "v" }),
    )
    .await;
    let empty_key = call(&tools, "store_value", json!({ "key": "", "value": "v" })).await;
    let long_value = call(
        &tools,
        "store_value",
        json!({ "key": "k", "value": "v".repeat(MAX_VALUE_BYTES + 1) }),
    )
    .await;
    let fits = call(
        &tools,
        "store_value",
        json!({ "key": "k".repeat(MAX_KEY_BYTES), "value": "v".repeat(MAX_VALUE_BYTES) }),
    )
    .await;

    assert!(long_key.contains("1 to 256 bytes"), "{long_key}");
    assert!(empty_key.contains("1 to 256 bytes"), "{empty_key}");
    assert!(long_value.contains("at most 65536 bytes"), "{long_value}");
    assert!(fits.starts_with("Stored key:"), "{fits}");
}

#[tokio::test]
async fn parallel_writes_do_not_lose_values() {
    let path = file("kv-parallel");
    let tools = tools(&path);
    let store = tools[0].clone();

    let calls = (0..20).map(|n| {
        let store = store.clone();
        async move {
            store
                .call_json(json!({ "key": format!("k{n:02}"), "value": "x" }))
                .await
        }
    });
    for result in futures_util::future::join_all(calls).await {
        result.unwrap();
    }

    assert_eq!(
        call(&tools, "list_keys", json!({})).await.lines().count(),
        20
    );
}
