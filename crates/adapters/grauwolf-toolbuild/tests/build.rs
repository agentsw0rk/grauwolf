//! Tool code from specs → WASM module following tool ABI v1 (ADR-0024, ADR-0029).

use std::path::PathBuf;

use futures_executor::block_on;
use grauwolf_sandbox::SandboxRuntime;
use grauwolf_spec::ToolSpec;
use grauwolf_toolbuild::{BuildError, ToolBuilder};
use grauwolf_tools::{ToolDescriptor, ToolRuntime};
use serde_json::json;

fn cache_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/toolbuild-tests")
}

/// Builder on the shared test cache, dependencies prepared (ADR-0030).
fn prepared() -> ToolBuilder {
    let builder = ToolBuilder::new(cache_dir());
    builder.prepare().unwrap();
    builder
}

fn add_tool() -> ToolSpec {
    ToolSpec {
        name: "add".into(),
        description: "Adds two numbers".into(),
        args: json!({ "type": "object", "properties": { "a": { "type": "number" }, "b": { "type": "number" } } }),
        code: r#"
            let a = args["a"].as_f64().ok_or("a missing")?;
            let b = args["b"].as_f64().ok_or("b missing")?;
            Ok((a + b).to_string())
        "#
        .into(),
    }
}

#[test]
#[ignore = "slow: builds tool code for WASM with cargo"]
fn builds_tool_code_into_a_module_the_sandbox_can_run() {
    let tool = add_tool();
    let artifact = prepared().build(&tool).unwrap();

    let runtime = SandboxRuntime::new().unwrap();
    let descriptor = ToolDescriptor {
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: tool.args.clone(),
    };
    let sandboxed = runtime.instantiate(&artifact, descriptor).unwrap();

    assert_eq!(
        block_on(sandboxed.call_json(json!({ "a": 2, "b": 3 }))).unwrap(),
        "5"
    );
}

#[test]
fn rejects_tool_code_that_could_reach_the_build_host() {
    let forbidden = [
        (
            r#"Ok(include_str!("/etc/passwd").to_string())"#,
            "include_str!",
        ),
        (r#"Ok(env!("HOME").to_string())"#, "env!"),
        (
            r#"#[path = "/etc/passwd"] mod secret; Ok(String::new())"#,
            "#[…]",
        ),
        ("mod secret; Ok(String::new())", "mod"),
        (
            "let x = unsafe { std::mem::zeroed::<u8>() }; Ok(x.to_string())",
            "unsafe",
        ),
        (
            r#"Ok(std::convert::identity(include_bytes ! ("/etc/hosts")).len().to_string())"#,
            "include_bytes!",
        ),
    ];

    for (code, construct) in forbidden {
        let tool = ToolSpec {
            code: code.into(),
            ..add_tool()
        };
        let error = prepared().build(&tool).unwrap_err();
        assert!(
            matches!(&error, BuildError::ForbiddenCode { construct: found, .. } if found == construct),
            "{code}: {error}"
        );
    }
}

#[test]
#[ignore = "slow: builds tool code for WASM with cargo"]
fn caches_artifacts_by_content() {
    let cache =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/toolbuild-cache-test");
    let _ = std::fs::remove_dir_all(cache.join("artifacts"));
    let builder = ToolBuilder::new(&cache);
    builder.prepare().unwrap();
    let tool = add_tool();
    assert_eq!(builder.cached(&tool).unwrap(), None);

    let built = builder.build(&tool).unwrap();

    assert!(!built.hash.is_empty());
    assert_eq!(builder.cached(&tool).unwrap(), Some(built));
    let changed = ToolSpec {
        code: format!("{}\n// changed", tool.code),
        ..tool
    };
    assert_eq!(builder.cached(&changed).unwrap(), None);
}

#[test]
fn stops_builds_that_exceed_the_time_limit() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let tool = ToolSpec {
        code: format!("let _ = {unique}_u128; Ok(String::new())"),
        ..add_tool()
    };
    let builder = prepared().with_timeout(std::time::Duration::from_millis(1));

    let error = builder.build(&tool).unwrap_err();

    assert!(matches!(error, BuildError::Timeout { .. }), "{error}");
}

#[test]
#[ignore = "slow: builds tool code for WASM with cargo"]
fn builds_offline_only_after_dependencies_were_prepared() {
    let cache =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/toolbuild-prepare-test");
    let _ = std::fs::remove_dir_all(&cache);
    let builder = ToolBuilder::new(&cache);

    let error = builder.build(&add_tool()).unwrap_err();
    assert!(matches!(error, BuildError::NotPrepared), "{error}");

    builder.prepare().unwrap();
    assert!(builder.build(&add_tool()).is_ok());
}

#[test]
#[ignore = "slow: builds tool code for WASM with cargo"]
fn runs_cargo_under_a_configured_isolation_wrapper() {
    let marker = cache_dir().join("wrapper-was-used");
    let _ = std::fs::remove_file(&marker);
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let tool = ToolSpec {
        code: format!("let _ = {unique}_u128; Ok(String::new())"),
        ..add_tool()
    };
    let wrapper = vec![
        "sh".to_string(),
        "-c".to_string(),
        format!("touch '{}' && exec \"$@\"", marker.display()),
        "wrapper".to_string(),
    ];

    prepared().with_wrapper(wrapper).build(&tool).unwrap();

    assert!(marker.exists(), "cargo did not run under the wrapper");
}

#[test]
#[ignore = "slow: builds tool code for WASM with cargo"]
fn builds_tools_concurrently_into_a_fresh_cache() {
    let cache = cache_dir().with_file_name("toolbuild-parallel-test");
    let _ = std::fs::remove_dir_all(&cache);
    let builder = ToolBuilder::new(&cache);
    builder.prepare().unwrap();
    // Two different tools, and the same tool twice (same content, same hash).
    let tools: Vec<ToolSpec> = ["tool0", "tool1", "same", "same"]
        .into_iter()
        .map(|name| ToolSpec {
            name: name.into(),
            ..add_tool()
        })
        .collect();

    let results: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = tools
            .iter()
            .map(|tool| scope.spawn(|| builder.build(tool)))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    for result in results {
        if let Err(error) = result {
            panic!("parallel build failed: {error}");
        }
    }
}

#[test]
#[ignore = "slow: builds tool code for WASM with cargo"]
fn reports_compile_errors_with_lines_relative_to_the_tool_code() {
    let tool = ToolSpec {
        name: "broken".into(),
        code: "let a = args[\"a\"].as_f64().unwrap_or(0.0);\nOk((a + unknown_value).to_string())"
            .into(),
        ..add_tool()
    };

    let error = prepared().build(&tool).unwrap_err();

    let BuildError::Compile { diagnostics, .. } = &error else {
        panic!("Expected a compile error: {error}");
    };
    let first = &diagnostics[0];
    assert_eq!(
        (first.level.as_str(), first.line, first.column),
        ("error", Some(2), Some(9))
    );
    assert!(first.message.contains("unknown_value"), "{first:?}");
    assert!(error.to_string().contains("line 2, column 9"), "{error}");
}

#[test]
fn reports_where_tool_code_cannot_be_tokenized() {
    let tool = ToolSpec {
        name: "escaped".into(),
        // Doubly escaped line break: the code contains a single backslash.
        code: "let a = 1;\\nOk(a.to_string())".into(),
        ..add_tool()
    };

    let error = prepared().build(&tool).unwrap_err();

    assert!(
        matches!(&error, BuildError::InvalidCode { reason, .. } if reason.contains("line 1, column 11")),
        "{error}"
    );
}

#[test]
fn writes_the_guest_crate_as_text_without_a_toolchain() {
    let tool = add_tool();
    let guest = grauwolf_toolbuild::guest_crate(&tool).unwrap();

    assert_eq!(guest.name, "add");
    assert!(guest.manifest.contains("name = \"gw_tool_add\""));
    assert!(guest.source.contains("let a = args[\"a\"]"));
    assert_eq!(
        guest.hash,
        grauwolf_toolbuild::source_hash(&guest.manifest, &guest.source)
    );
    assert_eq!(guest, grauwolf_toolbuild::guest_crate(&tool).unwrap());
    let mut changed = tool.clone();
    changed.code.push(' ');
    assert_ne!(
        guest.hash,
        grauwolf_toolbuild::guest_crate(&changed).unwrap().hash
    );
}

#[test]
fn refuses_to_write_forbidden_tool_code_as_sources() {
    let mut tool = add_tool();
    tool.code = "Ok(include_str!(\"/etc/passwd\").into())".into();
    let error = grauwolf_toolbuild::guest_crate(&tool).unwrap_err();
    assert!(matches!(error, BuildError::ForbiddenCode { .. }), "{error}");
}

#[test]
#[ignore = "slow: builds tool code for WASM with cargo"]
fn builds_written_sources_into_a_module_the_sandbox_can_run() {
    let tool = add_tool();
    let guest = grauwolf_toolbuild::guest_crate(&tool).unwrap();

    let artifact = prepared().build_sources(&guest).unwrap();

    let descriptor = ToolDescriptor {
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: tool.args.clone(),
    };
    let sandboxed = SandboxRuntime::new()
        .unwrap()
        .instantiate(&artifact, descriptor)
        .unwrap();
    assert_eq!(
        block_on(sandboxed.call_json(json!({ "a": 2, "b": 3 }))).unwrap(),
        "5"
    );
    assert_eq!(artifact.hash, guest.hash);
}

#[test]
#[ignore = "slow: builds tool code for WASM with cargo"]
fn reports_compile_errors_of_written_sources_relative_to_the_tool_code() {
    let mut tool = add_tool();
    tool.code = "let a: u32 = \"text\";\nOk(a.to_string())".into();
    let guest = grauwolf_toolbuild::guest_crate(&tool).unwrap();

    let error = prepared().build_sources(&guest).unwrap_err();

    let BuildError::Compile {
        tool, diagnostics, ..
    } = error
    else {
        panic!("{error}")
    };
    assert_eq!(tool, "add");
    assert_eq!(diagnostics[0].line, Some(1));
}
