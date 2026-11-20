//! The System One wire format: request body and response decoding (ADR-0100).
//!
//! Options and probabilities are ordered, and the order is part of the format, so they are
//! written and read as sequences of pairs instead of through `serde_json::Map`, which sorts.

use std::collections::HashMap;

use grauwolf_agents::decision::{Decision, DecisionKind, Verdict};
use serde::de::{MapAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Body of a request with one question `id` about `state`.
pub(crate) fn request<'a>(decision: &'a Decision, state: &'a Value, id: &'a str) -> Request<'a> {
    let (kind, criteria) = match &decision.kind {
        DecisionKind::Choice { options } => (
            "choice",
            Some(Criteria::Options(Pairs(
                options
                    .iter()
                    .map(|option| (option.name.as_str(), option.description.as_deref()))
                    .collect(),
            ))),
        ),
        DecisionKind::YesNo { yes, no, .. } => (
            "noul",
            (yes.is_some() || no.is_some()).then_some(Criteria::YesNo(YesNo {
                yes: yes.as_deref(),
                no: no.as_deref(),
            })),
        ),
        DecisionKind::Score { levels } => (
            "score",
            Some(Criteria::Levels(
                levels.iter().map(String::as_str).collect(),
            )),
        ),
    };
    Request {
        state,
        model: decision.model.as_deref(),
        questions: Questions {
            id,
            question: Question {
                kind,
                instructions: &decision.question,
                criteria,
            },
        },
    }
}

#[derive(Serialize)]
pub(crate) struct Request<'a> {
    state: &'a Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
    questions: Questions<'a>,
}

/// `{ "<id>": question }`.
struct Questions<'a> {
    id: &'a str,
    question: Question<'a>,
}

impl Serialize for Questions<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry(self.id, &self.question)?;
        map.end()
    }
}

#[derive(Serialize)]
struct Question<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    instructions: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    criteria: Option<Criteria<'a>>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Criteria<'a> {
    /// Option name → description or `null`, in the order of the options.
    Options(Pairs<'a>),
    /// Level descriptions, lowest first.
    Levels(Vec<&'a str>),
    YesNo(YesNo<'a>),
}

#[derive(Serialize)]
struct YesNo<'a> {
    #[serde(rename = "true", skip_serializing_if = "Option::is_none")]
    yes: Option<&'a str>,
    #[serde(rename = "false", skip_serializing_if = "Option::is_none")]
    no: Option<&'a str>,
}

/// An ordered JSON object.
struct Pairs<'a>(Vec<(&'a str, Option<&'a str>)>);

impl Serialize for Pairs<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, description) in &self.0 {
            map.serialize_entry(name, description)?;
        }
        map.end()
    }
}

/// The message of an error response `{ "error": { "type", "message" } }`, otherwise the text.
pub(crate) fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value["error"]["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| body.trim().chars().take(500).collect())
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    model: Option<String>,
    answers: HashMap<String, Answer>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Answer {
    Noul {
        noul: f64,
    },
    Choice {
        choice: String,
        #[serde(default)]
        confidence: Option<f64>,
        #[serde(default)]
        probabilities: Ordered<f64>,
    },
    Score {
        score: f64,
        #[serde(default)]
        confidence: Option<f64>,
        #[serde(default)]
        legend: Ordered<String>,
        #[serde(default)]
        probabilities: Ordered<f64>,
    },
}

impl Answer {
    fn name(&self) -> &'static str {
        match self {
            Answer::Noul { .. } => "noul",
            Answer::Choice { .. } => "choice",
            Answer::Score { .. } => "score",
        }
    }
}

/// A JSON object read in the order it was sent.
struct Ordered<V>(Vec<(String, V)>);

impl<V> Default for Ordered<V> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<'de, V: Deserialize<'de>> Deserialize<'de> for Ordered<V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Pairs<V>(std::marker::PhantomData<V>);
        impl<'de, V: Deserialize<'de>> Visitor<'de> for Pairs<V> {
            type Value = Ordered<V>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut pairs = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    pairs.push(entry);
                }
                Ok(Ordered(pairs))
            }
        }
        deserializer.deserialize_map(Pairs(std::marker::PhantomData))
    }
}

/// The verdict for `decision` from the response `body`; the answer of question `id` must
/// have the type of the decision.
pub(crate) fn verdict(decision: &Decision, body: &str, id: &str) -> Result<Verdict, String> {
    let mut response: Response = serde_json::from_str(body)
        .map_err(|error| format!("unreadable System One answer: {error}"))?;
    let answer = response
        .answers
        .remove(id)
        .ok_or_else(|| format!("System One answered without the question '{id}'"))?;
    let model = response.model;
    match (&decision.kind, answer) {
        (kind @ DecisionKind::YesNo { .. }, Answer::Noul { noul }) => Ok(Verdict {
            answer: kind.answer_for_probability(noul).to_owned(),
            probability: Some(noul),
            model,
            ..Verdict::default()
        }),
        (
            DecisionKind::Choice { .. },
            Answer::Choice {
                choice,
                confidence,
                probabilities,
            },
        ) => Ok(Verdict {
            answer: choice,
            confidence,
            probabilities: probabilities.0,
            model,
            ..Verdict::default()
        }),
        (
            kind @ DecisionKind::Score { levels },
            Answer::Score {
                score,
                confidence,
                legend,
                probabilities,
            },
        ) => {
            // The service names levels by index; the verdict names them by description.
            let describe = |index: &str| {
                index
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| levels.get(index).cloned())
                    .or_else(|| {
                        legend
                            .0
                            .iter()
                            .find(|(key, _)| key == index)
                            .map(|(_, text)| text.clone())
                    })
                    .unwrap_or_else(|| index.to_owned())
            };
            Ok(Verdict {
                answer: kind.answer_for_score(score).to_owned(),
                confidence,
                probabilities: probabilities
                    .0
                    .iter()
                    .map(|(index, probability)| (describe(index), *probability))
                    .collect(),
                model,
                ..Verdict::default()
            })
        }
        (kind, answer) => Err(format!(
            "System One answered a {} question with a '{}' answer",
            match kind {
                DecisionKind::Choice { .. } => "choice",
                DecisionKind::YesNo { .. } => "yes_no",
                DecisionKind::Score { .. } => "score",
            },
            answer.name()
        )),
    }
}
