//! The limits a tool package asks for (ADR-0124): the sandbox applies the stricter of the
//! package's and its own.

use std::time::{Duration, Instant};

use futures_executor::block_on;
use grauwolf_sandbox::{Limits, SandboxRuntime};
use grauwolf_tools::{ToolArtifact, ToolDescriptor, ToolLimits, ToolRuntime};
use serde_json::json;

fn artifact(wat: &str) -> ToolArtifact {
    ToolArtifact {
        name: "t".into(),
        bytes: wat.as_bytes().to_vec(),
        hash: "test".into(),
    }
}

fn descriptor() -> ToolDescriptor {
    ToolDescriptor {
        name: "t".into(),
        description: "Test tool".into(),
        parameters: json!({ "type": "object" }),
    }
}

/// Counts to 100 000, then answers `{"ok":"done"}`: well inside the host's default fuel.
const COUNTS: &str = r#"(module
  (memory (export "memory") 1)
  (data (i32.const 1024) "{\"ok\":\"done\"}")
  (func (export "gw_alloc") (param i32) (result i32) (i32.const 2048))
  (func (export "gw_call") (param i32 i32) (result i64) (local $i i32)
    (loop $again
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br_if $again (i32.lt_u (local.get $i) (i32.const 100000))))
    (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const 13))))"#;

/// Runs forever.
const ENDLESS: &str = r#"(module
  (memory (export "memory") 1)
  (func (export "gw_alloc") (param i32) (result i32) (i32.const 2048))
  (func (export "gw_call") (param i32 i32) (result i64)
    (loop $forever (br $forever))
    (i64.const 0)))"#;

/// Starts with 2 MiB of memory (32 pages of 64 KiB).
const BIG_MEMORY: &str = r#"(module
  (memory (export "memory") 32)
  (data (i32.const 1024) "{\"ok\":\"big\"}")
  (func (export "gw_alloc") (param i32) (result i32) (i32.const 2048))
  (func (export "gw_call") (param i32 i32) (result i64)
    (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const 12))))"#;

fn call(runtime: &SandboxRuntime, wat: &str, limits: ToolLimits) -> Result<String, String> {
    let tool = runtime
        .instantiate_limited(&artifact(wat), descriptor(), &limits)
        .unwrap();
    block_on(tool.call_json(json!({}))).map_err(|error| error.to_string())
}

#[test]
fn a_package_fuel_limit_below_the_hosts_stops_the_tool() {
    let runtime = SandboxRuntime::new().unwrap();
    assert_eq!(
        call(&runtime, COUNTS, ToolLimits::default()).unwrap(),
        "done"
    );

    let error = call(
        &runtime,
        COUNTS,
        ToolLimits {
            fuel: Some(10_000),
            ..ToolLimits::default()
        },
    )
    .unwrap_err();

    assert!(error.contains("compute budget"), "{error}");
}

#[test]
fn a_package_cannot_raise_the_hosts_fuel() {
    let runtime = SandboxRuntime::with_limits(Limits {
        fuel: 10_000,
        ..Limits::default()
    })
    .unwrap();
    let error = call(
        &runtime,
        COUNTS,
        ToolLimits {
            fuel: Some(u64::MAX),
            ..ToolLimits::default()
        },
    )
    .unwrap_err();
    assert!(error.contains("compute budget"), "{error}");
}

#[test]
fn a_package_memory_limit_below_the_hosts_refuses_a_bigger_memory() {
    let runtime = SandboxRuntime::new().unwrap();
    assert_eq!(
        call(&runtime, BIG_MEMORY, ToolLimits::default()).unwrap(),
        "big"
    );

    let error = call(
        &runtime,
        BIG_MEMORY,
        ToolLimits {
            memory_mib: Some(1),
            ..ToolLimits::default()
        },
    )
    .unwrap_err();

    assert!(error.contains("memory"), "{error}");
}

#[test]
fn a_package_time_limit_stops_a_tool_the_host_would_let_run_on() {
    let runtime = SandboxRuntime::with_limits(Limits {
        fuel: u64::MAX,
        ..Limits::default()
    })
    .unwrap();
    let started = Instant::now();

    let error = call(
        &runtime,
        ENDLESS,
        ToolLimits {
            time_ms: Some(50),
            ..ToolLimits::default()
        },
    )
    .unwrap_err();

    assert!(error.contains("time limit (50 ms)"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
}
