//! Code in graphs on the command line (ADR-0102): `spec build`, `spec generate --tool-sources`
//! and `tools build` treat `code` nodes, `on_code` conditions and edge transformations like tools.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const EXAMPLE: &str = "docs/user-guide/examples/04-code.json";

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

fn grauwolf(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(args)
        .current_dir(workspace())
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn work(name: &str) -> PathBuf {
    let dir = workspace().join("target/cli-tests").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cache() -> String {
    workspace()
        .join("target/cli-tests/code-cache")
        .display()
        .to_string()
}

fn generate(spec: &Path, out: &Path) -> Output {
    grauwolf(&[
        "spec",
        "generate",
        spec.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--tool-sources",
    ])
}

#[test]
fn tool_sources_include_the_code_of_the_graph_with_its_kind() {
    let out = work("code-sources").join("crate");

    let output = generate(&workspace().join(EXAMPLE), &out);

    assert!(output.status.success(), "{output:?}");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("tools/tools.json")).unwrap())
            .unwrap();
    let entries: Vec<(&str, &str)> = manifest["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["name"].as_str().unwrap(),
                entry["kind"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        entries,
        [
            ("code_main_score", "code"),
            ("condition_main_score_finish", "condition"),
            ("transform_main_score_finish", "transform"),
            ("transform_main_score_finish_2", "transform"),
        ]
    );
    let lib = std::fs::read_to_string(out.join("tools/code_main_score/src/lib.rs")).unwrap();
    assert!(
        lib.contains("fn run(input: In) -> Result<Out, String> {"),
        "{lib}"
    );
    assert!(lib.contains("pub struct Verdict"), "{lib}");
}

#[test]
#[ignore = "slow: builds code for WASM with cargo"]
fn tools_build_builds_the_code_of_the_graph_and_reports_its_kind() {
    let out = work("code-tools-build").join("crate");
    assert!(generate(&workspace().join(EXAMPLE), &out).status.success());

    let output = grauwolf(&[
        "tools",
        "build",
        out.to_str().unwrap(),
        "--cache",
        &cache(),
        "--format",
        "json",
    ]);

    assert!(output.status.success(), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["tools"][0]["name"], "code_main_score");
    assert_eq!(report["tools"][0]["kind"], "code");
    assert_eq!(report["tools"][0]["ok"], true);
    assert_eq!(report["tools"][2]["kind"], "transform");
    assert!(out.join("tools/code_main_score.wasm").is_file());
}

/// The example with the code of `score` replaced by `code`.
fn broken_example(name: &str, code: &str) -> PathBuf {
    let dir = work(name);
    let mut spec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(workspace().join(EXAMPLE)).unwrap()).unwrap();
    let params = &mut spec["graph"]["nodes"][1]["params"];
    params.as_object_mut().unwrap().remove("code_file");
    params["code"] = code.into();
    let file = dir.join("spec.json");
    std::fs::write(&file, spec.to_string()).unwrap();
    file
}

#[test]
#[ignore = "slow: builds code for WASM with cargo"]
fn compile_errors_name_the_node_in_spec_build_and_in_tools_build() {
    let file = broken_example("code-broken", "let a: u32 = \"x\";\nOk(unknown_value)");

    let built = grauwolf(&["spec", "build", file.to_str().unwrap(), "--cache", &cache()]);

    assert_eq!(built.status.code(), Some(1));
    let stderr = text(&built.stderr);
    assert!(
        stderr.contains("spec.json:/graph/nodes/1/params/code:2:"),
        "{stderr}"
    );

    let out = file.parent().unwrap().join("crate");
    assert!(generate(&file, &out).status.success());
    let sources = grauwolf(&[
        "tools",
        "build",
        out.to_str().unwrap(),
        "--cache",
        &cache(),
        "--format",
        "json",
    ]);
    assert_eq!(sources.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&sources.stdout).unwrap();
    assert_eq!(report["tools"][0]["name"], "code_main_score");
    assert_eq!(report["tools"][0]["errors"][0]["line"], 2);
}

#[test]
fn spec_validate_names_the_place_of_missing_code() {
    let file = broken_example("code-missing", "");

    let output = grauwolf(&["spec", "validate", file.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("/graph/nodes/1/params/code") && stderr.contains("code is missing"),
        "{stderr}"
    );
}
