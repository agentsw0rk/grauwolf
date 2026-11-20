//! Models: providers, fallbacks (ADR-0088) and sampling parameters (ADR-0097, ADR-0100).

use super::Rules;
use super::{ValidationError, ValidationErrorKind, escape_pointer_token};

/// Path, model (provider, ID) and its fallback models, for checking duplicates (ADR-0088).
type FallbackList = (String, (String, String), Vec<(String, String)>);

impl Rules<'_> {
    /// Fallback models (ADR-0088) of the model, of the main graph, of the subgraphs and of the
    /// `set_model`/`llm_moderate` nodes: none may equal the model or an earlier one;
    /// embedding models have none.
    pub(super) fn check_fallbacks(&self, errors: &mut Vec<ValidationError>) {
        let mut models = vec![("/model".to_owned(), &self.model)];
        for (name, subgraph) in &self.subgraphs {
            if let Some(model) = &subgraph.model {
                models.push((
                    format!("/subgraphs/{}/model", escape_pointer_token(name)),
                    model,
                ));
            }
        }
        let mut lists: Vec<FallbackList> = models
            .into_iter()
            .map(|(base, model)| {
                (
                    base,
                    (model.provider.clone(), model.id.clone()),
                    model
                        .fallbacks
                        .iter()
                        .map(|fallback| (fallback.provider.clone(), fallback.id.clone()))
                        .collect(),
                )
            })
            .collect();
        // Likewise the models of `set_model` and `llm_moderate`.
        let graphs = std::iter::once(("/graph".to_owned(), &self.graph)).chain(
            self.subgraphs.iter().map(|(name, subgraph)| {
                (
                    format!("/subgraphs/{}/graph", escape_pointer_token(name)),
                    &subgraph.graph,
                )
            }),
        );
        for (path, graph) in graphs {
            for (index, node) in graph.nodes.iter().enumerate() {
                let Some(params) = node.params.as_ref() else {
                    continue;
                };
                let text = |value: &serde_json::Value, key: &str| {
                    value.get(key).and_then(|v| v.as_str()).map(str::to_owned)
                };
                let (Some(provider), Some(id), Some(fallbacks)) = (
                    text(params, "provider"),
                    text(params, "id"),
                    params.get("fallbacks").and_then(|v| v.as_array()),
                ) else {
                    continue;
                };
                let fallbacks = fallbacks
                    .iter()
                    .filter_map(|fallback| {
                        Some((text(fallback, "provider")?, text(fallback, "id")?))
                    })
                    .collect();
                lists.push((
                    format!("{path}/nodes/{index}/params"),
                    (provider, id),
                    fallbacks,
                ));
            }
        }
        for (base, main, fallbacks) in &lists {
            let mut seen = vec![main.clone()];
            for (index, fallback) in fallbacks.iter().enumerate() {
                let key = fallback.clone();
                if seen.contains(&key) {
                    errors.push(ValidationError {
                        path: format!("{base}/fallbacks/{index}"),
                        kind: ValidationErrorKind::DuplicateFallback {
                            model: format!("{}:{}", fallback.0, fallback.1),
                        },
                    });
                }
                seen.push(key);
            }
        }
        let embeddings = [
            (
                "/knowledge/embedding_model",
                self.knowledge.as_ref().map(|k| &k.embedding_model),
            ),
            (
                "/long_term_memory/embedding_model",
                self.long_term_memory.as_ref().map(|m| &m.embedding_model),
            ),
        ];
        for (base, model) in embeddings {
            if model.is_some_and(|model| !model.fallbacks.is_empty()) {
                errors.push(ValidationError {
                    path: format!("{base}/fallbacks"),
                    kind: ValidationErrorKind::EmbeddingFallbacks,
                });
            }
        }
    }

    /// `temperature` (0–2) and `think` (only `ollama`) of every model (ADR-0097): the main model,
    /// subgraph models, their fallbacks, and the models of `set_model`/`llm_moderate` nodes;
    /// embedding models have neither.
    pub(super) fn check_sampling(&self, errors: &mut Vec<ValidationError>) {
        let mut system_one = Vec::new();
        let mut check =
            |base: &str, provider: &str, temperature: Option<f64>, think: Option<bool>| {
                if temperature.is_some_and(|value| !(0.0..=2.0).contains(&value)) {
                    errors.push(ValidationError {
                        path: format!("{base}/temperature"),
                        kind: ValidationErrorKind::TemperatureOutOfRange,
                    });
                }
                if think.is_some() && provider != "ollama" {
                    errors.push(ValidationError {
                        path: format!("{base}/think"),
                        kind: ValidationErrorKind::ThinkNotSupported {
                            provider: provider.to_owned(),
                        },
                    });
                }
            };
        let mut models = vec![("/model".to_owned(), &self.model)];
        for (name, subgraph) in &self.subgraphs {
            if let Some(model) = &subgraph.model {
                models.push((
                    format!("/subgraphs/{}/model", escape_pointer_token(name)),
                    model,
                ));
            }
        }
        for (base, model) in models {
            check(&base, &model.provider, model.temperature, model.think);
            for (index, fallback) in model.fallbacks.iter().enumerate() {
                check(
                    &format!("{base}/fallbacks/{index}"),
                    &fallback.provider,
                    fallback.temperature,
                    fallback.think,
                );
            }
        }
        let graphs = std::iter::once(("/graph".to_owned(), &self.graph)).chain(
            self.subgraphs.iter().map(|(name, subgraph)| {
                (
                    format!("/subgraphs/{}/graph", escape_pointer_token(name)),
                    &subgraph.graph,
                )
            }),
        );
        for (path, graph) in graphs {
            for (index, node) in graph.nodes.iter().enumerate() {
                if node.kind == "decide" {
                    let Some(params) = node.params.as_ref() else {
                        continue;
                    };
                    let base = format!("{path}/nodes/{index}/params");
                    if params.get("decider").and_then(|v| v.as_str()) == Some("system_one") {
                        for name in ["temperature", "think"] {
                            if params.get(name).is_some() {
                                system_one.push(ValidationError {
                                    path: format!("{base}/{name}"),
                                    kind: ValidationErrorKind::SamplingWithSystemOne,
                                });
                            }
                        }
                        continue;
                    }
                    // The decision request goes to the `model` parameter, else to the main model.
                    let provider = params
                        .get("model")
                        .and_then(|v| v.as_str())
                        .and_then(|model| model.split_once('/'))
                        .map_or(self.model.provider.as_str(), |(provider, _)| provider);
                    let temperature = params
                        .get("temperature")
                        .map(|v| v.as_f64().unwrap_or(f64::NAN));
                    let think = params.get("think").and_then(|v| v.as_bool());
                    check(&base, provider, temperature, think);
                    continue;
                }
                if node.kind != "set_model" && node.kind != "llm_moderate" {
                    continue;
                }
                let Some(params) = node.params.as_ref() else {
                    continue;
                };
                let base = format!("{path}/nodes/{index}/params");
                let entries = std::iter::once((base.clone(), params)).chain(
                    params
                        .get("fallbacks")
                        .and_then(|v| v.as_array())
                        .into_iter()
                        .flatten()
                        .enumerate()
                        .map(|(position, fallback)| {
                            (format!("{base}/fallbacks/{position}"), fallback)
                        }),
                );
                for (entry_path, entry) in entries {
                    let Some(provider) = entry.get("provider").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let temperature = entry
                        .get("temperature")
                        .map(|v| v.as_f64().unwrap_or(f64::NAN));
                    let think = entry.get("think").and_then(|v| v.as_bool());
                    check(&entry_path, provider, temperature, think);
                }
            }
        }
        errors.extend(system_one);
        let embeddings = [
            (
                "/knowledge/embedding_model",
                self.knowledge.as_ref().map(|k| &k.embedding_model),
            ),
            (
                "/long_term_memory/embedding_model",
                self.long_term_memory.as_ref().map(|m| &m.embedding_model),
            ),
        ];
        for (base, model) in embeddings {
            if model.is_some_and(|model| model.temperature.is_some() || model.think.is_some()) {
                errors.push(ValidationError {
                    path: base.to_owned(),
                    kind: ValidationErrorKind::EmbeddingSampling,
                });
            }
        }
    }
}
