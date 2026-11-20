//! Missing Rust toolchain: its own test file, because the test changes the process environment.

use grauwolf_spec::ToolSpec;
use grauwolf_toolbuild::{BuildError, ToolBuilder};
use serde_json::json;

#[test]
fn names_the_missing_program_when_the_toolchain_is_not_installed() {
    // SAFETY: the only test in this process; no other threads read the environment.
    unsafe { std::env::set_var("RUSTC", "/no/such/rustc") };
    let cache = grauwolf_test::TestDir::new("missing-toolchain");
    let builder = ToolBuilder::new(cache.path());
    let tool = ToolSpec {
        name: "noop".into(),
        description: "Does nothing".into(),
        args: json!({ "type": "object" }),
        code: "Ok(String::new())".into(),
    };

    let error = builder.build(&tool).unwrap_err();

    assert!(
        matches!(&error, BuildError::ToolchainMissing { program } if program == "/no/such/rustc"),
        "{error:?}"
    );
    assert!(error.to_string().contains("not found"), "{error}");
}
