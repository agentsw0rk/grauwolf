//! `packages::pack_folder`: a package folder becomes a deterministic OCI image layout.

use std::path::{Path, PathBuf};

use grauwolf_host::packages::pack_folder;
use grauwolf_toolbuild::ToolBuilder;
use grauwolf_toolpkg::PackedPackage;
use grauwolf_toolpkg::setup::pack_folder_built_at;

fn temp(name: &str) -> grauwolf_test::TestDir {
    grauwolf_test::TestDir::new(&format!("pack-folder-{name}"))
}

fn write_folder(dir: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("grauwolf-tools.json"),
        r#"{ "name": "math", "version": "1.0.0", "tools": [
            { "name": "add", "description": "Adds two numbers",
              "args": { "type": "object", "properties": { "a": {"type":"number"}, "b": {"type":"number"} } } } ] }"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("src/add.rs"),
        "let a = args[\"a\"].as_f64().ok_or(\"a is missing\")?;\n\
         let b = args[\"b\"].as_f64().ok_or(\"b is missing\")?;\n\
         Ok((a + b).to_string())\n",
    )
    .unwrap();
}

#[test]
#[ignore = "slow: builds tool code for WASM with cargo"]
fn the_same_folder_gives_the_same_digests_even_with_a_fresh_build() {
    let root = temp("determinism");
    let folder = root.join("pkg");
    write_folder(&folder);

    // The build date is part of the package (ADR-0124); the same date gives the same digest.
    let built = 1_700_000_000;
    let first =
        pack_folder_built_at(&folder, &ToolBuilder::new(root.join("cache-1")), built).unwrap();
    let second =
        pack_folder_built_at(&folder, &ToolBuilder::new(root.join("cache-2")), built).unwrap();

    assert_eq!(first.digest, second.digest);
    assert_eq!(first.blobs, second.blobs);
    let package = first.package().unwrap();
    assert_eq!(package.manifest.name, "math");
    assert!(package.modules["add.wasm"].bytes.starts_with(b"\0asm"));
    assert!(package.source.is_some());
    first.write_layout(&root.join("dist")).unwrap();
    assert_eq!(
        PackedPackage::read_layout(&root.join("dist")).unwrap(),
        first
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_tool_without_source_file_is_reported_with_the_path() {
    let root = temp("missing-source");
    write_folder(&root);
    std::fs::remove_file(root.join("src/add.rs")).unwrap();

    let error = pack_folder(&root, &ToolBuilder::new(root.join("cache"))).unwrap_err();

    assert!(error.contains("src/add.rs"), "{error}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
#[ignore = "slow: builds tool code for WASM with cargo"]
fn the_example_package_folder_of_the_user_guide_packs() {
    let root = temp("user-guide-example");
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../docs/user-guide/examples/03-package");

    let packed = pack_folder(&folder, &ToolBuilder::new(root.join("cache"))).unwrap();

    let package = packed.package().unwrap();
    assert_eq!(package.manifest.name, "text-and-math");
    assert!(package.modules["add.wasm"].bytes.starts_with(b"\0asm"));
    assert!(
        package.modules["count_words.wasm"]
            .bytes
            .starts_with(b"\0asm")
    );
    std::fs::remove_dir_all(&root).unwrap();
}
