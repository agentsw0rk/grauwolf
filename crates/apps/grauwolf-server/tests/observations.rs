//! Observations of a running agent for the signature (ADR-0094).

use grauwolf_prompt::ToolDescriptor;
use grauwolf_server::observations::schema_hash;
use serde_json::json;

fn tool(parameters: serde_json::Value) -> ToolDescriptor {
    ToolDescriptor {
        name: "search".into(),
        description: "Searches".into(),
        parameters,
    }
}

/// SHA-256 over `required(…);optional(…)` with `name:type:description` per parameter.
#[test]
fn hashes_required_and_optional_parameters() {
    let hash = schema_hash(&tool(json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "Search text" },
            "limit": { "type": "integer", "description": "Max results" }
        },
        "required": ["query"]
    })));

    // sha256("required(query:string:Search text);optional(limit:integer:Max results)")
    assert_eq!(
        hash,
        "1b94f37d555d5bd805e7a6cec2225393ac04f29a279a0de4136dff6515b1a069"
    );
}

#[test]
fn renders_enums_lists_unions_and_objects() {
    let hash = schema_hash(&tool(json!({
        "type": "object",
        "properties": {
            "mode": { "type": "string", "enum": ["fast", "slow"], "description": "Mode" },
            "tags": { "type": "array", "items": { "type": "string" }, "description": "Tags" },
            "value": { "anyOf": [ { "type": "string" }, { "type": "number" } ], "description": "Either" },
            "point": {
                "type": "object",
                "description": "A point",
                "properties": {
                    "x": { "type": "number", "description": "X" },
                    "y": { "type": "number", "description": "Y" }
                },
                "required": ["x"],
                "additionalProperties": false
            }
        }
    })));

    // sha256("required();optional(mode:enum[fast,slow]:Mode,point:object[x:number:X,y:number:Y;
    //   required=x;additional=false:]:A point,tags:list[string]:Tags,value:anyOf[string,number]:Either)")
    assert_eq!(
        hash,
        "5e6e9146b426081edcbd7094d6ccba831f9d1aa6f06ea9699ec48485c5b1f809"
    );
}

mod models {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, UNIX_EPOCH};

    use futures_executor::block_on;
    use grauwolf_agents::events::{AgentEvent, Feature};
    use grauwolf_prompt::{LlModel, Message};
    use grauwolf_server::observations::Observations;
    use serde_json::{Value, json};

    /// 2026-11-11T10:00:00Z plus the seconds in `clock`.
    fn observations(names: &[(&str, &str)], clock: &Arc<AtomicU64>) -> Arc<Observations> {
        let clock = Arc::clone(clock);
        Observations::with_clock(
            names
                .iter()
                .map(|(spec, name)| (spec.to_string(), name.to_string()))
                .collect::<HashMap<_, _>>(),
            Arc::new(move || {
                UNIX_EPOCH
                    + Duration::from_secs(1_794_391_200)
                    + Duration::from_secs(clock.load(Ordering::SeqCst))
            }),
        )
    }

    fn answered(feature: &dyn Feature, requested: LlModel, reported: Option<&str>) {
        feature.on_event(&AgentEvent::LlmCallStarting {
            model: requested,
            tools: vec![],
        });
        feature.on_event(&AgentEvent::LlmCallCompleted {
            answer: Message::assistant("Hi"),
            usage: None,
            cost: None,
            model: reported.map(str::to_owned),
        });
    }

    fn report(observations: &Observations) -> Value {
        block_on(observations.report(None))
    }

    #[test]
    fn reports_the_start_and_the_model_per_agent_with_first_and_last_seen() {
        let clock = Arc::new(AtomicU64::new(0));
        let observations = observations(&[], &clock);
        let feature = observations.feature_for("helper");
        clock.store(30, Ordering::SeqCst);

        answered(
            &*feature,
            LlModel::new("openai", "gpt-4o-mini"),
            Some("gpt-4o-mini-2024-07-18"),
        );

        let report = report(&observations);
        assert_eq!(report["startedAt"], "2026-11-11T10:00:00.000Z");
        assert_eq!(
            report["models"],
            json!({ "helper": [ {
                "modelId": "gpt-4o-mini-2024-07-18",
                "firstSeen": "2026-11-11T10:00:30.000Z",
                "lastSeen": "2026-11-11T10:00:30.000Z"
            } ] })
        );
        assert_eq!(report["mcpTools"], json!({}));
    }

    #[test]
    fn a_repeated_value_moves_last_seen_and_a_new_value_adds_an_entry() {
        let clock = Arc::new(AtomicU64::new(0));
        let observations = observations(&[], &clock);
        let feature = observations.feature_for("helper");
        let requested = || LlModel::new("openai", "gpt-4o-mini");

        clock.store(10, Ordering::SeqCst);
        answered(&*feature, requested(), Some("gpt-4o-mini-2024-07-18"));
        clock.store(20, Ordering::SeqCst);
        answered(&*feature, requested(), Some("gpt-4o-mini-2025-01-01"));
        clock.store(30, Ordering::SeqCst);
        answered(&*feature, requested(), Some("gpt-4o-mini-2024-07-18"));

        let report = report(&observations);
        assert_eq!(
            report["models"]["helper"],
            json!([
                { "modelId": "gpt-4o-mini-2024-07-18",
                  "firstSeen": "2026-11-11T10:00:10.000Z", "lastSeen": "2026-11-11T10:00:30.000Z" },
                { "modelId": "gpt-4o-mini-2025-01-01",
                  "firstSeen": "2026-11-11T10:00:20.000Z", "lastSeen": "2026-11-11T10:00:20.000Z" }
            ])
        );
    }

    #[test]
    fn uses_the_mapped_name_of_the_agent_and_the_spec_name_without_a_mapping() {
        let clock = Arc::new(AtomicU64::new(0));
        let observations = observations(&[("main", "Support Agent")], &clock);

        answered(
            &*observations.feature_for("main"),
            LlModel::new("openai", "m"),
            Some("m-1"),
        );
        answered(
            &*observations.feature_for("helper"),
            LlModel::new("openai", "m"),
            Some("m-1"),
        );

        let report = report(&observations);
        let agents: Vec<&String> = report["models"].as_object().unwrap().keys().collect();
        assert_eq!(agents, ["Support Agent", "helper"]);
    }

    #[test]
    fn a_provider_that_reports_no_model_gives_null() {
        let clock = Arc::new(AtomicU64::new(0));
        let observations = observations(&[], &clock);

        answered(
            &*observations.feature_for("helper"),
            LlModel::new("openai", "m"),
            None,
        );

        assert_eq!(
            report(&observations)["models"]["helper"][0]["modelId"],
            Value::Null
        );
    }

    /// Digest source that counts its lookups.
    struct Tags(AtomicU64, Option<&'static str>);

    impl grauwolf_server::observations::ModelDigests for Tags {
        fn digest<'a>(
            &'a self,
            _model: &'a str,
        ) -> futures_util::future::BoxFuture<'a, Option<String>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(std::future::ready(self.1.map(str::to_owned)))
        }
    }

    /// `<model>@<first 12 hex of the digest>`; the lookup is cached for ten minutes.
    #[test]
    fn names_an_ollama_model_with_the_start_of_its_digest_and_caches_the_lookup() {
        let clock = Arc::new(AtomicU64::new(0));
        let observations = observations(&[], &clock);
        let feature = observations.feature_for("helper");
        let tags = Tags(AtomicU64::new(0), Some("sha256:0123456789abcdef0123"));
        answered(
            &*feature,
            LlModel::new("ollama", "qwen3.5:9b"),
            Some("qwen3.5:9b"),
        );

        let first = block_on(observations.report(Some(&tags)));
        clock.store(599, Ordering::SeqCst);
        answered(
            &*feature,
            LlModel::new("ollama", "qwen3.5:9b"),
            Some("qwen3.5:9b"),
        );
        block_on(observations.report(Some(&tags)));
        clock.store(601, Ordering::SeqCst);
        answered(
            &*feature,
            LlModel::new("ollama", "qwen3.5:9b"),
            Some("qwen3.5:9b"),
        );
        block_on(observations.report(Some(&tags)));

        assert_eq!(
            first["models"]["helper"][0]["modelId"],
            "qwen3.5:9b@0123456789ab"
        );
        assert_eq!(tags.0.load(Ordering::SeqCst), 2);
    }

    /// Unknown digest: the reported model, else null.
    #[test]
    fn falls_back_to_the_reported_model_when_the_digest_is_unknown() {
        let clock = Arc::new(AtomicU64::new(0));
        let observations = observations(&[], &clock);
        let feature = observations.feature_for("helper");
        answered(&*feature, LlModel::new("ollama", "a"), Some("a"));
        answered(&*feature, LlModel::new("ollama", "b"), None);

        let report = block_on(observations.report(Some(&Tags(AtomicU64::new(0), None))));

        let ids: Vec<&Value> = report["models"]["helper"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| &entry["modelId"])
            .collect();
        assert_eq!(ids, [&json!("a"), &Value::Null]);
    }
}

mod mcp_tools {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, UNIX_EPOCH};

    use futures_executor::block_on;
    use grauwolf_prompt::ToolDescriptor;
    use grauwolf_server::observations::Observations;
    use serde_json::json;

    fn tool(name: &str, description: &str) -> ToolDescriptor {
        ToolDescriptor {
            name: name.into(),
            description: description.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search text" },
                    "limit": { "type": "integer", "description": "Max results" }
                },
                "required": ["query"]
            }),
        }
    }

    #[test]
    fn lists_the_tools_per_server_sorted_by_name_and_only_adds_an_entry_when_they_change() {
        let clock = Arc::new(AtomicU64::new(0));
        let ticks = Arc::clone(&clock);
        let observations = Observations::with_clock(
            HashMap::new(),
            Arc::new(move || {
                UNIX_EPOCH
                    + Duration::from_secs(1_794_391_200)
                    + Duration::from_secs(ticks.load(Ordering::SeqCst))
            }),
        );

        clock.store(5, Ordering::SeqCst);
        observations.record_mcp_tools("docs", &[tool("search", "Searches"), tool("fetch", "Gets")]);
        clock.store(65, Ordering::SeqCst);
        observations.record_mcp_tools("docs", &[tool("fetch", "Gets"), tool("search", "Searches")]);
        clock.store(125, Ordering::SeqCst);
        observations.record_mcp_tools("docs", &[tool("fetch", "Gets"), tool("search", "Finds")]);

        let report = block_on(observations.report(None));
        let hash = "1b94f37d555d5bd805e7a6cec2225393ac04f29a279a0de4136dff6515b1a069";
        assert_eq!(
            report["mcpTools"],
            json!({ "docs": [
                { "tools": [
                    { "name": "fetch", "description": "Gets", "schemaHash": hash },
                    { "name": "search", "description": "Searches", "schemaHash": hash }
                  ],
                  "firstSeen": "2026-11-11T10:00:05.000Z",
                  "lastSeen": "2026-11-11T10:01:05.000Z" },
                { "tools": [
                    { "name": "fetch", "description": "Gets", "schemaHash": hash },
                    { "name": "search", "description": "Finds", "schemaHash": hash }
                  ],
                  "firstSeen": "2026-11-11T10:02:05.000Z",
                  "lastSeen": "2026-11-11T10:02:05.000Z" }
            ] })
        );
    }
}
