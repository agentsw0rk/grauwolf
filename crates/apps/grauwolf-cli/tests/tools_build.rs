//! `grauwolf tools build`: the modules of a crate generated with `--tool-sources` (ADR-0095).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

/// Generates the calculator spec with the code of its `add` tool replaced by `code` (if given).
fn generate(name: &str, code: Option<&str>) -> PathBuf {
    let dir = work(name);
    let mut spec: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(workspace().join("examples/specs/calculator.json")).unwrap(),
    )
    .unwrap();
    if let Some(code) = code {
        spec["tools"][0]["code"] = code.into();
    }
    std::fs::write(dir.join("spec.json"), spec.to_string()).unwrap();
    let out = dir.join("crate");
    let output = grauwolf(&[
        "spec",
        "generate",
        dir.join("spec.json").to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--tool-sources",
    ]);
    assert!(output.status.success(), "{output:?}");
    out
}

fn build(out: &Path, extra: &[&str]) -> Output {
    let cache = workspace().join("target/cli-tests/tools-build-cache");
    let mut args = vec![
        "tools",
        "build",
        out.to_str().unwrap(),
        "--cache",
        cache.to_str().unwrap(),
    ];
    args.extend_from_slice(extra);
    grauwolf(&args)
}

#[test]
fn reports_a_missing_manifest() {
    let dir = work("tools-build-missing");

    let output = grauwolf(&["tools", "build", dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = text(&output.stderr);
    assert!(stderr.contains("tools/tools.json"), "{stderr}");
    assert!(stderr.contains("--tool-sources"), "{stderr}");
}

#[test]
fn refuses_tool_sources_that_changed_after_generation() {
    let out = generate("tools-build-changed", None);
    let lib = out.join("tools/add/src/lib.rs");
    let source = std::fs::read_to_string(&lib).unwrap();
    std::fs::write(&lib, source.replace("a + b", "a - b")).unwrap();

    let output = build(&out, &[]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = text(&output.stderr);
    assert!(stderr.contains("add"), "{stderr}");
    assert!(stderr.contains("changed"), "{stderr}");
    assert!(!out.join("tools/add.wasm").exists());
}

#[test]
fn reports_a_changed_source_in_json_per_tool() {
    let out = generate("tools-build-changed-json", None);
    std::fs::write(out.join("tools/add/Cargo.toml"), "[package]\n").unwrap();

    let output = build(&out, &["--format", "json"]);

    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["tools"][0]["name"], "add");
    assert_eq!(report["tools"][0]["ok"], false);
    assert!(
        report["tools"][0]["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("changed")
    );
}

#[test]
#[ignore = "slow: builds tool code for WASM with cargo"]
fn builds_the_modules_and_skips_them_the_second_time() {
    let out = generate("tools-build-ok", None);

    let first = build(&out, &[]);
    assert!(first.status.success(), "{first:?}");
    assert!(text(&first.stdout).contains("add: built tools/add.wasm"));
    let module = std::fs::read(out.join("tools/add.wasm")).unwrap();
    assert_eq!(&module[..4], b"\0asm");

    let second = build(&out, &[]);
    assert!(second.status.success(), "{second:?}");
    assert!(text(&second.stdout).contains("up to date"), "{second:?}");

    let forced = build(&out, &["--force"]);
    assert!(text(&forced.stdout).contains("built"), "{forced:?}");
}

#[test]
#[ignore = "slow: builds tool code for WASM with cargo"]
fn reports_compile_errors_per_tool_with_the_line_of_the_tool_code() {
    let out = generate(
        "tools-build-error",
        Some("let a: u32 = \"text\";\nOk(a.to_string())"),
    );

    let text_output = build(&out, &[]);
    assert_eq!(text_output.status.code(), Some(1));
    assert!(
        text(&text_output.stderr).contains("add: line 1, column"),
        "{text_output:?}"
    );

    let json = build(&out, &["--format", "json"]);
    assert_eq!(json.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(report["tools"][0]["name"], "add");
    assert_eq!(report["tools"][0]["ok"], false);
    assert_eq!(report["tools"][0]["errors"][0]["line"], 1);
    assert!(report["tools"][0]["errors"][0]["column"].is_number());
    assert!(!out.join("tools/add.wasm").exists());
}
