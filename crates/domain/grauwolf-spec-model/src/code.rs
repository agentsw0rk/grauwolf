//! Code in graphs (ADR-0102, F14.14): the parameters of the node `code` and of the condition
//! `on_code`, the edge transformation and the code pieces they make, shared by the validation,
//! the tool build, the interpreter and the generator.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Node kind that runs Rust code on its value (ADR-0102): `params { input, output, code }`.
/// Like `for_each` it needs the built module, so it is not in the node catalog.
pub const CODE_KIND: &str = "code";

/// Condition kind that decides with Rust code (ADR-0102): `params { input, code }`.
pub const ON_CODE_KIND: &str = "on_code";

/// Transformation on an edge (ADR-0102): code that maps the value to the input of the target.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransformSpec {
    /// Type of the value that the transformation reads; without it, the type that the edge
    /// delivers (the output of the source node, after the condition).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    /// Type of the result; the target node receives it.
    pub output: String,
    /// Body of `fn transform(value: In) -> Out`.
    #[serde(default)]
    pub code: String,
}

/// The parameters of a `code` node.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeNodeParams {
    /// Input type.
    pub input: String,
    /// Output type.
    pub output: String,
    /// Body of `fn run(input: In) -> Result<Out, String>`.
    #[serde(default)]
    pub code: String,
}

impl CodeNodeParams {
    /// Reads the parameters of a node; the error says what is wrong.
    pub fn parse(params: Option<&Value>) -> Result<Self, String> {
        let params = params.ok_or("missing parameters \"input\", \"output\" and \"code\"")?;
        serde_json::from_value(params.clone()).map_err(|error| error.to_string())
    }
}

/// The parameters of an `on_code` condition.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnCodeParams {
    /// Type of the value; must be the output of the source node.
    pub input: String,
    /// Body of `fn check(value: &In) -> bool`.
    #[serde(default)]
    pub code: String,
}

impl OnCodeParams {
    /// Reads the parameters of a condition; the error says what is wrong.
    pub fn parse(params: Option<&Value>) -> Result<Self, String> {
        let params = params.ok_or("missing parameters \"input\" and \"code\"")?;
        serde_json::from_value(params.clone()).map_err(|error| error.to_string())
    }
}

/// What a code piece is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodePieceKind {
    /// `fn run(input: In) -> Result<Out, String>` of a `code` node.
    Node,
    /// `fn check(value: &In) -> bool` of an `on_code` condition.
    Condition,
    /// `fn transform(value: In) -> Out` of an edge.
    Transform,
}

impl CodePieceKind {
    /// Name in `tools/tools.json` and prefix of the piece names: `code`, `condition`,
    /// `transform`.
    pub fn as_str(self) -> &'static str {
        match self {
            CodePieceKind::Node => "code",
            CodePieceKind::Condition => "condition",
            CodePieceKind::Transform => "transform",
        }
    }
}

/// A piece of code of a graph, ready to be built like a tool (ADR-0102).
#[derive(Debug, Clone, PartialEq)]
pub struct CodePiece {
    /// Stable name derived from the node or edge: `code_<scope>_<node>`,
    /// `condition_<scope>_<from>_<to>`, `transform_<scope>_<from>_<to>`, where `<scope>` is
    /// `main` or `sub_<subgraph>`; a second edge between the same nodes gets `_2` and so on.
    pub name: String,
    /// What the piece is.
    pub kind: CodePieceKind,
    /// JSON pointer to the code in the spec, e.g. `/graph/nodes/1/params/code`; compile errors
    /// point there.
    pub pointer: String,
    /// Input type, e.g. `string`, `Review` or `array<Review>`.
    pub input: String,
    /// Output type; `bool` for a condition.
    pub output: String,
    /// The code body.
    pub code: String,
    /// The named types the piece uses (name → JSON schema).
    pub types: BTreeMap<String, Value>,
}
