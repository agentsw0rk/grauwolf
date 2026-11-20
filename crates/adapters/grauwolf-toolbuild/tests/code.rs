//! Code of graphs → WASM module (ADR-0102): the same build, cache and ABI as tools.

use std::collections::BTreeMap;
use std::path::PathBuf;

use futures_executor::block_on;
use grauwolf_sandbox::{Limits, SandboxRuntime};
use grauwolf_spec::{CodePiece, CodePieceKind};
use grauwolf_toolbuild::{BuildError, ToolBuilder};
use grauwolf_tools::{DynTool, ToolDescriptor, ToolRuntime, ToolSourceKind};
use serde_json::{Value, json};

fn prepared() -> ToolBuilder {
    let builder = ToolBuilder::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/toolbuild-tests"),
    );
    builder.prepare().unwrap();
    builder
}

fn types() -> BTreeMap<String, Value> {
    BTreeMap::from([
        (
            "Review".into(),
            json!({ "type": "object", "properties": {
                "text": { "type": "string" },
                "stars": { "type": "integer" },
                "tags": { "type": "array", "items": { "type": "string" } },
                "type": { "type": "string" },
                "note": { "type": "string" }
            }, "required": ["text", "stars", "tags"] }),
        ),
        (
            "Summary".into(),
            json!({ "type": "object", "properties": {
                "headline": { "type": "string" }, "score": { "type": "number" }, "good": { "type": "boolean" }
            }, "required": ["headline", "score", "good"] }),
        ),
    ])
}

fn piece(name: &str, kind: CodePieceKind, input: &str, output: &str, code: &str) -> CodePiece {
    CodePiece {
        name: name.into(),
        kind,
        pointer: "/graph/nodes/0/params/code".into(),
        input: input.into(),
        output: output.into(),
        code: code.into(),
        types: types(),
    }
}

fn sandboxed(artifact: &grauwolf_tools::ToolArtifact) -> std::sync::Arc<dyn DynTool> {
    sandboxed_with(artifact, SandboxRuntime::new().unwrap())
}

fn sandboxed_with(
    artifact: &grauwolf_tools::ToolArtifact,
    runtime: SandboxRuntime,
) -> std::sync::Arc<dyn DynTool> {
    runtime
        .instantiate(
            artifact,
            ToolDescriptor {
                name: artifact.name.clone(),
                description: String::new(),
                parameters: json!({}),
            },
        )
        .unwrap()
}

fn node() -> CodePiece {
    piece(
        "code_main_summarize",
        CodePieceKind::Node,
        "Review",
        "Summary",
        r#"
        if input.stars < 0 {
            return Err(format!("negative stars: {}", input.stars));
        }
        Ok(Summary {
            headline: format!("{} ({})", input.text, input.tags.join(",")),
            score: input.stars as f64 / 2.0,
            good: input.stars >= 4,
        })"#,
    )
}

#[test]
fn a_code_piece_is_written_as_a_guest_crate_of_its_kind_without_a_toolchain() {
    let guest = grauwolf_toolbuild::code_guest_crate(&node()).unwrap();

    assert_eq!(guest.name, "code_main_summarize");
    assert_eq!(guest.kind, ToolSourceKind::Code);
    assert!(
        guest
            .manifest
            .contains("name = \"gw_tool_code_main_summarize\"")
    );
    assert!(
        guest
            .source
            .contains("fn run(input: In) -> Result<Out, String> {")
    );
    assert_eq!(
        guest.hash,
        grauwolf_toolbuild::source_hash(&guest.manifest, &guest.source)
    );
    let condition = piece("c", CodePieceKind::Condition, "Review", "bool", "true");
    assert_eq!(
        grauwolf_toolbuild::code_guest_crate(&condition)
            .unwrap()
            .kind,
        ToolSourceKind::Condition
    );
    let transform = piece("t", CodePieceKind::Transform, "string", "string", "value");
    assert_eq!(
        grauwolf_toolbuild::code_guest_crate(&transform)
            .unwrap()
            .kind,
        ToolSourceKind::Transform
    );
}

#[test]
fn code_pieces_are_checked_for_forbidden_constructs_like_tools() {
    let mut forbidden = node();
    forbidden.code = r#"Ok(include_str!("/etc/passwd").len())"#.into();

    let error = grauwolf_toolbuild::code_guest_crate(&forbidden).unwrap_err();

    assert!(
        matches!(&error, BuildError::ForbiddenCode { tool, construct } if tool == "code_main_summarize" && construct == "include_str!"),
        "{error}"
    );
}

#[test]
#[ignore = "slow: builds code for WASM with cargo"]
fn a_code_node_runs_in_the_sandbox_on_typed_json() {
    let artifact = prepared().build_code(&node()).unwrap();

    let run = |input: Value| block_on(sandboxed(&artifact).call_json(input));

    let output =
        run(json!({ "text": "fine", "stars": 5, "tags": ["a", "b"], "type": "x" })).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap(),
        json!({ "headline": "fine (a,b)", "score": 2.5, "good": true })
    );
    // `Err` fails the call with its message; a value of the wrong type is refused.
    let error = run(json!({ "text": "bad", "stars": -1, "tags": [] })).unwrap_err();
    assert!(error.to_string().contains("negative stars: -1"), "{error}");
    let error = run(json!({ "text": "bad", "stars": "many", "tags": [] })).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("field `stars`: expected an integer"),
        "{error}"
    );
    let error = run(json!({ "text": "bad", "stars": 1 })).unwrap_err();
    assert!(
        error.to_string().contains("missing field `tags`"),
        "{error}"
    );
}

#[test]
#[ignore = "slow: builds code for WASM with cargo"]
fn a_condition_answers_true_or_false_and_a_panic_is_an_error() {
    let check = piece(
        "condition_main_a_b",
        CodePieceKind::Condition,
        "Review",
        "bool",
        r#"if value.text == "boom" { panic!("boom") } value.stars >= 4 && value.note.is_none()"#,
    );
    let artifact = prepared().build_code(&check).unwrap();
    let tool = sandboxed(&artifact);

    let ask = |stars: i64, text: &str| {
        block_on(tool.call_json(json!({ "text": text, "stars": stars, "tags": [] })))
    };

    assert_eq!(ask(5, "x").unwrap(), "true");
    assert_eq!(ask(1, "x").unwrap(), "false");
    assert!(ask(5, "boom").is_err(), "a panic is an error, not false");
}

#[test]
#[ignore = "slow: builds code for WASM with cargo"]
fn a_transformation_reshapes_a_value_and_works_on_lists() {
    let reshape = piece(
        "transform_main_a_b",
        CodePieceKind::Transform,
        "array<Review>",
        "Summary",
        r#"Summary { headline: value.iter().map(|r| r.text.clone()).collect::<Vec<_>>().join("+"), score: value.len() as f64, good: !value.is_empty() }"#,
    );
    let artifact = prepared().build_code(&reshape).unwrap();

    let output = block_on(sandboxed(&artifact).call_json(json!([
        { "text": "a", "stars": 1, "tags": [] }, { "text": "b", "stars": 2, "tags": [] }
    ])))
    .unwrap();

    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap(),
        json!({ "headline": "a+b", "score": 2.0, "good": true })
    );
}

#[test]
#[ignore = "slow: builds code for WASM with cargo"]
fn code_that_never_ends_runs_out_of_fuel() {
    let endless = piece(
        "code_main_endless",
        CodePieceKind::Node,
        "string",
        "string",
        "loop { std::hint::black_box(1); }",
    );
    let artifact = prepared().build_code(&endless).unwrap();
    let runtime = SandboxRuntime::with_limits(Limits {
        fuel: 1_000_000,
        ..Limits::default()
    })
    .unwrap();

    let error = block_on(sandboxed_with(&artifact, runtime).call_json(json!("x"))).unwrap_err();

    assert!(error.to_string().contains("fuel"), "{error}");
}

#[test]
#[ignore = "slow: builds code for WASM with cargo"]
fn compile_errors_name_the_piece_and_the_line_in_its_code() {
    let mut broken = node();
    broken.code = "let a: u32 = 1;\nOk(unknown_value)".into();

    let error = prepared().build_code(&broken).unwrap_err();

    let BuildError::Compile {
        tool, diagnostics, ..
    } = error
    else {
        panic!("{error}")
    };
    assert_eq!(tool, "code_main_summarize");
    assert_eq!(diagnostics[0].line, Some(2));
}

#[test]
#[ignore = "slow: builds code for WASM with cargo"]
fn a_second_build_of_the_same_piece_comes_from_the_cache() {
    let builder = prepared();
    let first = builder.build_code(&node()).unwrap();

    let cached = builder.cached_code(&node()).unwrap().unwrap();

    assert_eq!(cached, first);
    let mut changed = node();
    changed.code.push(' ');
    assert!(builder.cached_code(&changed).unwrap().is_none());
}
