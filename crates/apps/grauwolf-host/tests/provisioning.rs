//! Provisioning through its interface (ADR-0114): `MapEnv`, a fake embedder, a fake supplier.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_host::config::MapEnv;
use grauwolf_host::provisioning::{ProvisionOptions, ProvisionedAgent, SubAgent, SubAgentSupplier};
use grauwolf_rag::{Embedder, Embedding, RagError};
use grauwolf_spec::{AgentSpec, AgentSpecExt};
use grauwolf_tools::DynTool;
use serde_json::{Value, json};

/// Counts the words of a fixed vocabulary – enough to test similarity.
struct WordCounter;

impl Embedder for WordCounter {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Embedding>, RagError>> {
        let embeddings = texts
            .iter()
            .map(|text| {
                let text = text.to_lowercase();
                Embedding(
                    ["cat", "wolf", "holiday"]
                        .iter()
                        .map(|word| text.matches(word).count() as f32)
                        .collect(),
                )
            })
            .collect();
        Box::pin(async move { Ok(embeddings) })
    }
}

fn base(name: &str) -> grauwolf_test::TestDir {
    grauwolf_test::TestDir::new(&format!("prov-{name}"))
}

fn spec(extra: Value) -> AgentSpec {
    let mut document = json!({
        "spec_version": 1, "name": "prov",
        "model": { "provider": "ollama", "id": "m" },
        "strategy": { "kind": "single_run" }
    });
    for (key, value) in extra.as_object().unwrap() {
        document[key] = value.clone();
    }
    AgentSpec::from_json(&document.to_string()).unwrap()
}

fn options() -> ProvisionOptions {
    ProvisionOptions::new(Arc::new(MapEnv::new([]))).with_embedder(Arc::new(WordCounter))
}

fn memory_spec() -> Value {
    json!({ "long_term_memory": {
        "store": "memory.json",
        "embedding_model": { "provider": "ollama", "id": "e" },
        "top_k": 1, "tools": true
    } })
}

fn tool<'a>(tools: &'a [Arc<dyn DynTool>], name: &str) -> &'a Arc<dyn DynTool> {
    tools.iter().find(|tool| tool.name() == name).unwrap()
}

#[tokio::test]
async fn memory_is_separate_per_user() {
    let base = base("memory");
    let agent = ProvisionedAgent::new(&spec(memory_spec()), &base, options())
        .await
        .unwrap();
    let anna = agent.run(Some("anna")).unwrap().tools().await;
    let bob = agent.run(Some("bob")).unwrap().tools().await;
    tool(&anna, "remember")
        .call_json(json!({ "fact": "Anna has a cat." }))
        .await
        .unwrap();

    let found_anna = tool(&anna, "recall")
        .call_json(json!({ "query": "cat" }))
        .await
        .unwrap();
    let found_bob = tool(&bob, "recall")
        .call_json(json!({ "query": "cat" }))
        .await
        .unwrap();

    assert!(found_anna.contains("Anna has a cat"), "{found_anna}");
    assert!(!found_bob.contains("Anna"), "{found_bob}");
}

#[tokio::test]
async fn facts_come_only_with_automatic_memory() {
    let base = base("facts");
    let plain = ProvisionedAgent::new(&spec(memory_spec()), &base, options())
        .await
        .unwrap();
    let mut auto = memory_spec();
    auto["long_term_memory"]["auto_recall"] = json!(true);
    let auto = ProvisionedAgent::new(&spec(auto), &base, options())
        .await
        .unwrap();

    assert!(plain.run(Some("anna")).unwrap().facts().is_none());
    assert!(auto.run(Some("anna")).unwrap().facts().is_some());
}

#[tokio::test]
async fn the_key_value_store_belongs_to_the_user() {
    let base = base("kv");
    let spec = spec(
        json!({ "builtin_tools": { "tools": ["store_value", "retrieve_value"], "root": "." } }),
    );
    let agent = ProvisionedAgent::new(&spec, &base, options())
        .await
        .unwrap();
    let anna = agent.run(Some("anna")).unwrap().tools().await;
    let bob = agent.run(Some("bob")).unwrap().tools().await;
    tool(&anna, "store_value")
        .call_json(json!({ "key": "color", "value": "red" }))
        .await
        .unwrap();

    let anna_value = tool(&anna, "retrieve_value")
        .call_json(json!({ "key": "color" }))
        .await
        .unwrap();
    let bob_value = tool(&bob, "retrieve_value")
        .call_json(json!({ "key": "color" }))
        .await
        .unwrap();

    assert!(anna_value.contains("red"), "{anna_value}");
    assert!(!bob_value.contains("red"), "{bob_value}");
}

#[tokio::test]
async fn knowledge_is_indexed_once_for_all_runs() {
    let base = base("knowledge");
    std::fs::write(base.join("wolves.md"), "The wolf is fed at eight.").unwrap();
    let spec = spec(json!({ "knowledge": {
        "sources": ["wolves.md"], "tool": "search_docs",
        "embedding_model": { "provider": "ollama", "id": "e" }
    } }));
    let agent = ProvisionedAgent::new(&spec, &base, options())
        .await
        .unwrap();
    std::fs::remove_file(base.join("wolves.md")).unwrap();

    let tools = agent.tools();
    let found = tool(&tools, "search_docs")
        .call_json(json!({ "query": "wolf" }))
        .await
        .unwrap();

    assert!(found.contains("fed at eight"), "{found}");
}

#[tokio::test]
async fn a_tool_group_works_from_the_given_environment_only() {
    let base = base("command-env");
    let spec = spec(json!({ "builtin_tools": {
        "tools": ["run_command"], "root": ".",
        "command": { "allowed": ["printenv"], "env": ["PATH", "PROVISIONING_ONLY_IN_MAP"] }
    } }));
    let env = MapEnv::new([
        ("PATH", "/usr/bin:/bin"),
        ("PROVISIONING_ONLY_IN_MAP", "from-the-map"),
    ]);
    let agent = ProvisionedAgent::new(
        &spec,
        &base,
        ProvisionOptions::new(Arc::new(env)).with_embedder(Arc::new(WordCounter)),
    )
    .await
    .unwrap();
    let tools = agent.tools();

    let output = tool(&tools, "run_command")
        .call_json(json!({ "program": "printenv", "args": ["PROVISIONING_ONLY_IN_MAP"] }))
        .await
        .unwrap();

    assert!(output.contains("from-the-map"), "{output}");
}

#[tokio::test]
async fn a_missing_credential_fails_before_the_first_run() {
    let base = base("credentials");
    let spec = spec(json!({ "builtin_tools": {
        "tools": ["jira_search_issues"], "jira": { "base_url": "https://example.invalid" }
    } }));

    let error = ProvisionedAgent::new(&spec, &base, options())
        .await
        .err()
        .unwrap();

    assert!(error.contains("ATLASSIAN_EMAIL"), "{error}");
}

/// Sub-agents that report the user they were called for.
struct Supplier;

impl SubAgentSupplier for Supplier {
    fn supply<'a>(
        &'a self,
        _: &'a AgentSpec,
        _: &'a std::path::Path,
    ) -> BoxFuture<'a, Result<Vec<SubAgent>, String>> {
        Box::pin(async {
            Ok(vec![SubAgent::new("helper", |user| {
                Arc::new(Helper(user.map(str::to_owned))) as Arc<dyn DynTool>
            })])
        })
    }
}

struct Helper(Option<String>);

impl DynTool for Helper {
    fn name(&self) -> &str {
        "helper"
    }

    fn descriptor(&self) -> grauwolf_prompt::ToolDescriptor {
        grauwolf_prompt::ToolDescriptor {
            name: "helper".into(),
            description: "test".into(),
            parameters: json!({ "type": "object" }),
        }
    }

    fn call_json(&self, _: Value) -> BoxFuture<'_, Result<String, grauwolf_tools::ToolError>> {
        Box::pin(async move { Ok(format!("user: {:?}", self.0)) })
    }
}

fn sub_agent_spec() -> AgentSpec {
    spec(json!({ "agent_tools": { "helper": { "spec": "helper.json" } } }))
}

#[tokio::test]
async fn sub_agents_run_with_the_user_of_the_calling_run() {
    let base = base("sub");
    let options = options().with_sub_agents(Arc::new(Supplier));
    let agent = ProvisionedAgent::new(&sub_agent_spec(), &base, options)
        .await
        .unwrap();
    let tools = agent.run(Some("anna")).unwrap().tools().await;

    let answer = tool(&tools, "helper").call_json(json!({})).await.unwrap();

    assert!(answer.contains("Some(\"anna\")"), "{answer}");
}

#[tokio::test]
async fn sub_agents_without_a_supplier_are_an_error() {
    let base = base("nosupplier");

    let error = ProvisionedAgent::new(&sub_agent_spec(), &base, options())
        .await
        .err()
        .unwrap();

    assert!(error.contains("supplier"), "{error}");
}

#[tokio::test]
async fn one_list_of_tools_per_user_for_callers_that_hand_them_on_themselves() {
    let base = base("tools-for");
    let spec = spec(
        json!({ "builtin_tools": { "tools": ["store_value", "retrieve_value"], "root": "." } }),
    );
    let agent = ProvisionedAgent::new(&spec, &base, options())
        .await
        .unwrap();
    let anna = agent.tools_for(Some("anna")).unwrap();
    let bob = agent.tools_for(Some("bob")).unwrap();
    tool(&anna, "store_value")
        .call_json(json!({ "key": "color", "value": "red" }))
        .await
        .unwrap();

    let bob_value = tool(&bob, "retrieve_value")
        .call_json(json!({ "key": "color" }))
        .await
        .unwrap();

    assert_eq!(anna.len(), bob.len());
    assert!(!bob_value.contains("red"), "{bob_value}");
}

#[tokio::test]
async fn the_price_table_of_the_options_comes_with_the_provisioned_agent() {
    let base = base("prices");
    let prices = Arc::new(grauwolf_prompt::Prices {
        currency: "EUR".into(),
        models: Default::default(),
    });
    let priced = ProvisionedAgent::new(
        &spec(json!({})),
        &base,
        options().with_prices(Some(Arc::clone(&prices))),
    )
    .await
    .unwrap();
    let plain = ProvisionedAgent::new(&spec(json!({})), &base, options())
        .await
        .unwrap();

    assert_eq!(priced.prices().unwrap().currency, "EUR");
    assert!(plain.prices().is_none());
}

#[tokio::test]
async fn the_price_table_file_of_the_environment_is_read_relative_to_the_base_directory() {
    let base = base("prices-file");
    std::fs::write(
        base.join("prices.json"),
        r#"{ "currency": "CHF", "models": {} }"#,
    )
    .unwrap();
    let env = MapEnv::new([("GRAUWOLF_PRICES", "prices.json")]);
    let agent = ProvisionedAgent::new(
        &spec(json!({})),
        &base,
        ProvisionOptions::new(Arc::new(env)),
    )
    .await
    .unwrap();

    assert_eq!(agent.prices().unwrap().currency, "CHF");
}

#[tokio::test]
async fn a_cost_limit_without_a_price_table_fails_before_the_first_run() {
    let base = base("cost-limit");
    let limited = spec(json!({ "limits": { "max_cost": 0.5 } }));

    let error = ProvisionedAgent::new(&limited, &base, options())
        .await
        .err()
        .unwrap();
    assert!(
        error.contains("`limits.max_cost` needs a price table"),
        "{error}"
    );

    let priced = options().with_prices(Some(Arc::new(grauwolf_prompt::Prices {
        currency: "USD".into(),
        models: Default::default(),
    })));
    assert!(ProvisionedAgent::new(&limited, &base, priced).await.is_ok());
}
