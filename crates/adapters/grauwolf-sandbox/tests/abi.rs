//! Sandbox following tool ABI v1 (ADR-0029).

use futures_executor::block_on;
use grauwolf_sandbox::{Limits, SandboxRuntime};
use grauwolf_tools::{ToolArtifact, ToolDescriptor, ToolRuntime};
use serde_json::json;

/// Always returns `{"ok":"pong"}`.
const PONG: &str = r#"(module
  (memory (export "memory") 1)
  (data (i32.const 1024) "{\"ok\":\"pong\"}")
  (func (export "gw_alloc") (param i32) (result i32) (i32.const 2048))
  (func (export "gw_call") (param i32 i32) (result i64)
    (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const 13))))"#;

pub fn artifact(name: &str, wat: &str) -> ToolArtifact {
    ToolArtifact {
        name: name.into(),
        bytes: wat.as_bytes().to_vec(),
        hash: "test".into(),
    }
}

pub fn descriptor(name: &str) -> ToolDescriptor {
    ToolDescriptor {
        name: name.into(),
        description: "Test tool".into(),
        parameters: json!({ "type": "object" }),
    }
}

#[test]
fn runs_a_module_that_follows_the_tool_abi() {
    let runtime = SandboxRuntime::new().unwrap();
    let tool = runtime
        .instantiate(&artifact("ping", PONG), descriptor("ping"))
        .unwrap();

    assert_eq!(tool.name(), "ping");
    assert_eq!(block_on(tool.call_json(json!({}))).unwrap(), "pong");
}

/// `gw_call` runs forever.
const ENDLESS: &str = r#"(module
  (memory (export "memory") 1)
  (func (export "gw_alloc") (param i32) (result i32) (i32.const 2048))
  (func (export "gw_call") (param i32 i32) (result i64)
    (loop $forever (br $forever))
    (i64.const 0)))"#;

#[test]
fn stops_endless_tool_code_when_the_fuel_is_used_up() {
    let runtime = SandboxRuntime::with_limits(Limits {
        fuel: 100_000,
        ..Limits::default()
    })
    .unwrap();
    let tool = runtime
        .instantiate(&artifact("loop", ENDLESS), descriptor("loop"))
        .unwrap();

    let error = block_on(tool.call_json(json!({}))).unwrap_err();

    assert!(error.to_string().contains("compute budget"), "{error}");
}

/// Wants to import a host function.
const WANTS_HOST: &str = r#"(module
  (import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "gw_alloc") (param i32) (result i32) (i32.const 2048))
  (func (export "gw_call") (param i32 i32) (result i64) (i64.const 0)))"#;

#[test]
fn rejects_modules_that_import_anything() {
    let runtime = SandboxRuntime::new().unwrap();

    let error = runtime
        .instantiate(&artifact("io", WANTS_HOST), descriptor("io"))
        .err()
        .expect("module with imports was loaded");

    assert!(
        error
            .to_string()
            .contains("wasi_snapshot_preview1::fd_write"),
        "{error}"
    );
}
