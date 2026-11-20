//! Package format `grauwolf-tools.json` (ADR-0107).

use grauwolf_tools::{PackageError, PackageManifest};

fn manifest(tools: &str) -> String {
    format!(
        r#"{{ "format": 1, "name": "acme-math", "version": "1.0.0", "abi": 1, "tools": {tools} }}"#
    )
}

const ADD: &str = r#"{ "name": "add", "description": "Adds two numbers", "module": "add.wasm",
    "args": { "type": "object", "properties": { "a": { "type": "number" }, "b": { "type": "number" } } } }"#;

#[test]
fn parses_a_valid_manifest() {
    let parsed = PackageManifest::from_json(&manifest(&format!("[{ADD}]"))).unwrap();
    assert_eq!(parsed.name, "acme-math");
    assert_eq!(parsed.version, "1.0.0");
    assert_eq!(parsed.tools.len(), 1);
    assert_eq!(parsed.tools[0].name, "add");
    assert_eq!(parsed.tools[0].module, "add.wasm");
}

#[test]
fn rejects_an_unknown_abi_version() {
    let json = manifest(&format!("[{ADD}]")).replace("\"abi\": 1", "\"abi\": 2");
    assert_eq!(
        PackageManifest::from_json(&json).unwrap_err(),
        PackageError::UnsupportedAbi { found: 2 }
    );
}

#[test]
fn rejects_an_unknown_format_version() {
    let json = manifest(&format!("[{ADD}]")).replace("\"format\": 1", "\"format\": 7");
    assert_eq!(
        PackageManifest::from_json(&json).unwrap_err(),
        PackageError::UnsupportedFormat { found: 7 }
    );
}

#[test]
fn rejects_a_package_without_tools() {
    assert_eq!(
        PackageManifest::from_json(&manifest("[]")).unwrap_err(),
        PackageError::NoTools
    );
}

#[test]
fn rejects_duplicate_tool_names() {
    let error = PackageManifest::from_json(&manifest(&format!("[{ADD}, {ADD}]"))).unwrap_err();
    assert_eq!(error, PackageError::DuplicateTool { name: "add".into() });
}

#[test]
fn rejects_tool_names_that_are_not_snake_case() {
    let json = manifest(&format!("[{ADD}]")).replace("\"add\"", "\"Add-It\"");
    assert_eq!(
        PackageManifest::from_json(&json).unwrap_err(),
        PackageError::InvalidToolName {
            name: "Add-It".into()
        }
    );
}

#[test]
fn rejects_module_names_that_leave_the_package() {
    for module in ["../evil.wasm", "sub/add.wasm", "/abs.wasm", ""] {
        let json = manifest(&format!("[{ADD}]")).replace("add.wasm", module);
        assert_eq!(
            PackageManifest::from_json(&json).unwrap_err(),
            PackageError::InvalidModuleName {
                module: module.into()
            },
            "{module}"
        );
    }
}

#[test]
fn rejects_unknown_fields_and_bad_json_with_a_message() {
    let json = manifest(&format!("[{ADD}]")).replace("\"abi\": 1", "\"abi\": 1, \"extra\": true");
    assert!(matches!(
        PackageManifest::from_json(&json).unwrap_err(),
        PackageError::Format(message) if message.contains("extra")
    ));
    assert!(matches!(
        PackageManifest::from_json("{").unwrap_err(),
        PackageError::Format(_)
    ));
}

#[test]
fn round_trips_through_json() {
    let parsed = PackageManifest::from_json(&manifest(&format!("[{ADD}]"))).unwrap();
    assert_eq!(
        PackageManifest::from_json(&parsed.to_json()).unwrap(),
        parsed
    );
}

#[test]
fn a_package_folder_manifest_may_omit_the_module_file_names() {
    let source = r#"{ "name": "acme-math", "version": "1.0.0",
        "tools": [{ "name": "add", "description": "Adds", "args": { "type": "object" } }] }"#;

    let manifest = PackageManifest::from_source_json(source).unwrap();

    assert_eq!(manifest.format, grauwolf_tools::PACKAGE_FORMAT);
    assert_eq!(manifest.abi, grauwolf_tools::TOOL_ABI);
    assert_eq!(manifest.tools[0].module, "add.wasm");
}

#[test]
fn a_package_folder_manifest_may_carry_metadata_and_limits() {
    let source = r#"{ "name": "acme-math", "version": "1.0.0",
        "description": "Arithmetic for agents", "license": "Apache-2.0",
        "authors": ["Ada <ada@example.com>"], "source": "https://github.com/acme/math",
        "tools": [{ "name": "add", "description": "Adds", "args": { "type": "object" },
            "limits": { "memory_mib": 16, "fuel": 1000000, "time_ms": 500 },
            "capabilities": [] }] }"#;

    let manifest = PackageManifest::from_source_json(source).unwrap();

    assert_eq!(
        manifest.description.as_deref(),
        Some("Arithmetic for agents")
    );
    assert_eq!(manifest.license.as_deref(), Some("Apache-2.0"));
    assert_eq!(manifest.authors, ["Ada <ada@example.com>"]);
    assert_eq!(
        manifest.source.as_deref(),
        Some("https://github.com/acme/math")
    );
    let limits = &manifest.tools[0].limits;
    assert_eq!(
        (limits.memory_mib, limits.fuel, limits.time_ms),
        (Some(16), Some(1_000_000), Some(500))
    );
    assert_eq!(
        PackageManifest::from_json(&manifest.to_json()).unwrap(),
        manifest
    );
}

#[test]
fn a_manifest_without_metadata_is_written_as_before() {
    let parsed = PackageManifest::from_json(&manifest(&format!("[{ADD}]"))).unwrap();
    let json = parsed.to_json();
    for key in ["license", "authors", "\"source\"", "limits", "capabilities"] {
        assert!(!json.contains(key), "{key} in {json}");
    }
}

#[test]
fn abi_1_grants_no_capabilities() {
    let json = manifest(&format!("[{ADD}]")).replace(
        "\"module\": \"add.wasm\"",
        "\"module\": \"add.wasm\", \"capabilities\": [\"http\"]",
    );
    assert_eq!(
        PackageManifest::from_json(&json).unwrap_err(),
        PackageError::UnsupportedCapabilities { tool: "add".into() }
    );
}

#[test]
fn a_limit_of_zero_is_refused() {
    let json = manifest(&format!("[{ADD}]")).replace(
        "\"module\": \"add.wasm\"",
        "\"module\": \"add.wasm\", \"limits\": { \"fuel\": 0 }",
    );
    assert_eq!(
        PackageManifest::from_json(&json).unwrap_err(),
        PackageError::InvalidLimit {
            tool: "add".into(),
            limit: "fuel".into()
        }
    );
}
