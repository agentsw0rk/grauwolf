//! `grauwolf-tools new` (ADR-0124): the scaffold of a tool package folder.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn work(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("tools-new-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn tools(args: &[&str], dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_grauwolf-tools"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

#[test]
fn new_writes_a_package_folder_that_validates() {
    let dir = work("validate");
    let created = tools(&["new", "text-tools", "--tool", "greet"], &dir);
    assert!(created.status.success(), "{created:?}");
    let folder = dir.join("text-tools");
    for file in [
        "grauwolf-tools.json",
        "src/greet.rs",
        "tests/greet.rs",
        "tests/support/build.rs",
        "tests/support/lib.rs",
        "Cargo.toml",
        "README.md",
        ".github/workflows/grauwolf-tools.yml",
    ] {
        assert!(folder.join(file).is_file(), "{file} missing");
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(folder.join("grauwolf-tools.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["name"], "text-tools");
    for field in ["description", "license", "authors"] {
        assert!(!manifest[field].is_null(), "{field} missing");
    }
    assert_eq!(manifest["tools"][0]["name"], "greet");
    let description = manifest["tools"][0]["description"].as_str().unwrap();
    assert!(
        description.contains("`greet`") && !description.contains("Greets"),
        "the description is a neutral placeholder naming the tool: {description}"
    );
    assert!(
        manifest["tools"][0]["args"]["properties"]["input"].is_object(),
        "{manifest}"
    );
    // Only the tool's test file is a test target of `cargo test`.
    assert!(!folder.join("tests/build.rs").exists() && !folder.join("tests/lib.rs").exists());
    let workflow =
        std::fs::read_to_string(folder.join(".github/workflows/grauwolf-tools.yml")).unwrap();
    assert!(
        workflow.contains("container: ghcr.io/agentsw0rk/grauwolf-tools")
            && workflow.contains("--sign-key"),
        "{workflow}"
    );

    let validated = tools(&["validate", "text-tools"], &dir);
    assert!(validated.status.success(), "{validated:?}");
}

#[test]
fn new_refuses_bad_names_and_a_folder_that_is_not_empty() {
    let dir = work("refuse");
    let bad = tools(&["new", "Text Tools"], &dir);
    assert_eq!(bad.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("package name"),
        "{bad:?}"
    );
    std::fs::create_dir_all(dir.join("taken")).unwrap();
    std::fs::write(dir.join("taken/file"), "x").unwrap();
    let taken = tools(&["new", "taken"], &dir);
    assert_eq!(taken.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&taken.stderr).contains("not empty"),
        "{taken:?}"
    );
}

#[test]
#[ignore = "slow: builds the scaffolded tool for WASM and runs its cargo test"]
fn a_new_package_builds_packs_and_passes_its_test() {
    let dir = work("pack");
    assert!(tools(&["new", "greetings"], &dir).status.success());
    let folder = dir.join("greetings");
    let packed = tools(&["pack", "greetings", "--cache", "cache"], &dir);
    assert!(packed.status.success(), "{packed:?}");
    assert!(folder.join("dist/oci-layout").is_file());
    let tested = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["test", "--offline"])
        .env("CARGO_TARGET_DIR", dir.join("target"))
        .current_dir(&folder)
        .output()
        .unwrap();
    assert!(tested.status.success(), "{tested:?}");
    // The scaffold's own tests ran (the example's two), and no empty test target is listed.
    let output = String::from_utf8_lossy(&tested.stdout);
    assert!(output.contains("test result: ok. 2 passed"), "{output}");
    assert!(!output.contains("running 0 tests"), "{output}");
}
