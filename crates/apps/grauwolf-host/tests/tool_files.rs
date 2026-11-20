//! Tool code and tool definitions in their own files (ADR-0073).

use grauwolf_host::loader::load_spec;
use serde_json::json;

/// Fresh directory in the temporary directory of the container.
fn dir(name: &str) -> grauwolf_test::TestDir {
    let dir = grauwolf_test::TestDir::new(name);
    std::fs::create_dir_all(dir.join("tools")).unwrap();
    dir
}

fn spec_with_tools(tools: serde_json::Value) -> serde_json::Value {
    json!({
        "spec_version": 1, "name": "calc",
        "model": { "provider": "ollama", "id": "m" },
        "tools": tools,
        "strategy": { "kind": "single_run" }
    })
}

#[test]
fn reads_tool_code_from_a_file_next_to_the_spec() {
    let dir = dir("code-file");
    let code = "let a = args[\"a\"].as_f64().ok_or(\"a missing\")?;\nOk((a * 2.0).to_string())";
    std::fs::write(dir.join("tools/double.rs"), code).unwrap();
    std::fs::write(
        dir.join("spec.json"),
        spec_with_tools(json!([{
            "name": "double", "description": "Doubles a",
            "args": { "type": "object", "properties": { "a": { "type": "number" } } },
            "code_file": "tools/double.rs"
        }]))
        .to_string(),
    )
    .unwrap();

    let spec = load_spec(&dir.join("spec.json")).unwrap();

    assert_eq!(spec.tools[0].code, code);
}

#[test]
fn reports_missing_code_files_and_code_given_twice_with_their_place() {
    let dir = dir("code-file-errors");
    let tool = |extra: serde_json::Value| {
        let mut tool = json!({
            "name": "double", "description": "Doubles a",
            "args": { "type": "object" }
        });
        for (key, value) in extra.as_object().unwrap() {
            tool[key] = value.clone();
        }
        tool
    };
    std::fs::write(dir.join("tools/double.rs"), "Ok(String::new())").unwrap();
    let write = |name: &str, tools: serde_json::Value| {
        let path = dir.join(name);
        std::fs::write(&path, spec_with_tools(tools).to_string()).unwrap();
        path
    };
    let missing = write(
        "missing.json",
        json!([tool(json!({ "code_file": "tools/missing.rs" }))]),
    );
    let twice = write(
        "twice.json",
        json!([tool(
            json!({ "code": "Ok(String::new())", "code_file": "tools/double.rs" })
        )]),
    );

    let missing = load_spec(&missing).unwrap_err();
    let twice = load_spec(&twice).unwrap_err();

    assert!(
        missing.contains("/tools/0/code_file") && missing.contains("missing.rs"),
        "{missing}"
    );
    assert!(
        twice.contains("/tools/0") && twice.contains("exactly one"),
        "{twice}"
    );
}

/// Tool library (ADR-0073): whole definitions from files, `code_file` relative to the
/// tool file.
#[test]
fn appends_tool_definitions_from_their_own_files() {
    let dir = dir("tool-files");
    let library = dir.join("library");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::write(
        library.join("upper.rs"),
        "Ok(args[\"text\"].as_str().unwrap_or(\"\").to_uppercase())",
    )
    .unwrap();
    std::fs::write(
        library.join("upper.tool.json"),
        json!({
            "name": "upper", "description": "Converts text to upper case",
            "args": { "type": "object", "properties": { "text": { "type": "string" } } },
            "code_file": "upper.rs"
        })
        .to_string(),
    )
    .unwrap();
    let mut spec = spec_with_tools(json!([{
        "name": "echo", "description": "Returns the text",
        "args": { "type": "object" }, "code": "Ok(args.to_string())"
    }]));
    spec["tool_files"] = json!(["library/upper.tool.json"]);
    std::fs::write(dir.join("spec.json"), spec.to_string()).unwrap();
    let mut missing = spec.clone();
    missing["tool_files"] = json!(["library/missing.tool.json"]);
    std::fs::write(dir.join("missing.json"), missing.to_string()).unwrap();

    let spec = load_spec(&dir.join("spec.json")).unwrap();
    let missing = load_spec(&dir.join("missing.json")).unwrap_err();

    let names: Vec<&str> = spec.tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(names, ["echo", "upper"]);
    assert!(spec.tools[1].code.contains("to_uppercase"));
    assert!(
        missing.contains("/tool_files/0") && missing.contains("missing.tool.json"),
        "{missing}"
    );
}

/// ADR-0080: errors with a location uniformly appear as `<file>:<pointer>: <message>`.
#[test]
fn reports_errors_with_a_pointer_as_file_colon_pointer() {
    let dir = grauwolf_test::TestDir::new("located");
    let typo = dir.join("typo.json");
    std::fs::write(
        &typo,
        json!({
            "spec_version": 1, "name": "agent", "sytem_prompt": "Hello",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let missing = dir.join("missing_file.json");
    std::fs::write(
        &missing,
        json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "tools": [ { "name": "add", "description": "Adds", "args": {}, "code_file": "missing.rs" } ],
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();

    let typo_error = grauwolf_host::loader::load_spec(&typo).unwrap_err();
    let missing_error = grauwolf_host::loader::load_spec(&missing).unwrap_err();

    assert!(
        typo_error.starts_with(&format!("{}:/sytem_prompt: unknown field", typo.display())),
        "{typo_error}"
    );
    assert!(
        missing_error.starts_with(&format!("{}:/tools/0/code_file: ", missing.display())),
        "{missing_error}"
    );
}
