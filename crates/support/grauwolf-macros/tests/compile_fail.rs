//! Wrong uses of `#[tool]` and `#[toolset]` fail to compile with an understandable message
//! (ADR-0003). A small crate with all failing cases is checked with `cargo check`; the test
//! only looks for the macros' own messages, not the wording of rustc.

use std::path::{Path, PathBuf};
use std::process::Command;

/// One failing case each, and the message that the macro emits for it.
const CASES: &[(&str, &str)] = &[
    (
        "#[tool(name = \"x\")]\nfn with_arguments() -> Result<String, ToolError> { Ok(String::new()) }",
        "#[tool] takes no arguments",
    ),
    (
        "#[tool]\nfn without_result() -> String { String::new() }",
        "#[tool] functions return `Result<T, ToolError>`",
    ),
    (
        "#[tool]\nfn with_pattern((a, b): (i32, i32)) -> Result<i32, ToolError> { Ok(a + b) }",
        "arguments need simple names",
    ),
    (
        "struct Shared;\n#[toolset(x)]\nimpl Shared {}",
        "#[toolset] takes no arguments",
    ),
    (
        "struct Described;\n#[toolset]\nimpl std::fmt::Display for Described {\n    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, \"\") }\n}",
        "#[toolset] belongs on an inherent `impl` block, not on a trait implementation",
    ),
    (
        "struct Generic<T>(T);\n#[toolset]\nimpl<T> Generic<T> {}",
        "#[toolset] does not support generic `impl` blocks",
    ),
    (
        "struct Mutable;\n#[toolset]\nimpl Mutable {\n    #[tool]\n    fn change(&mut self) -> Result<u32, ToolError> { Ok(1) }\n}",
        "tool methods in #[toolset] take `&self` (shared, not `&mut self`)",
    ),
];

fn crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Writes the test crate with all cases, each in its own module.
fn write_crate(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"macro_misuse\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [workspace]\n\n[dependencies]\ngrauwolf-macros = {{ path = {:?} }}\n\
             grauwolf-tools = {{ path = {:?} }}\n",
            crates_dir()
                .join("support/grauwolf-macros")
                .display()
                .to_string(),
            crates_dir()
                .join("domain/grauwolf-tools")
                .display()
                .to_string(),
        ),
    )
    .unwrap();
    let modules: String = CASES
        .iter()
        .enumerate()
        .map(|(index, (code, _))| {
            format!(
                "mod case_{index} {{\n    #![allow(dead_code)]\n    use grauwolf_macros::{{tool, toolset}};\n    use grauwolf_tools::ToolError;\n{code}\n}}\n\n"
            )
        })
        .collect();
    std::fs::write(dir.join("src/lib.rs"), modules).unwrap();
}

#[test]
#[ignore = "slow: checks a crate with cargo"]
fn misuse_fails_to_compile_with_a_clear_message() {
    let base = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| crates_dir().join("../target"))
        .join("macro-compile-fail");
    let dir = base.join("macro_misuse");
    write_crate(&dir);

    let output = Command::new(env!("CARGO"))
        .args(["check", "--quiet", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", base.join("target"))
        .env("CARGO_INCREMENTAL", "0")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "failing cases compile:\n{stderr}");
    for (code, message) in CASES {
        assert!(
            stderr.contains(message),
            "message missing: {message}\ncase:\n{code}\noutput:\n{stderr}"
        );
    }
}
