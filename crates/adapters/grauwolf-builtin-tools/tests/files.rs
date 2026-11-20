//! File tools under a root (F03.14, ADR-0046).

use std::path::PathBuf;
use std::sync::Arc;

use grauwolf_builtin_tools::FileTools;
use grauwolf_tools::DynTool;
use serde_json::json;

/// Fresh directory `name` under `target/builtin-tools-tests` with a file `notes.md`.
fn root(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/builtin-tools-tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("notes.md"), "Hello wolf").unwrap();
    root
}

fn tool(tools: &[Arc<dyn DynTool>], name: &str) -> Arc<dyn DynTool> {
    tools
        .iter()
        .find(|tool| tool.name() == name)
        .cloned()
        .unwrap()
}

#[tokio::test]
async fn reads_files_below_the_root_and_refuses_paths_leaving_it() {
    let root = root("read");
    std::fs::write(root.join("../secret.txt"), "geheim").unwrap();
    let tools = FileTools::new(&root).unwrap().tools(["read_file"]).unwrap();
    let read = tool(&tools, "read_file");

    let content = read.call_json(json!({ "path": "notes.md" })).await.unwrap();
    let escaped = read
        .call_json(json!({ "path": "../secret.txt" }))
        .await
        .unwrap_err();
    let absolute = read
        .call_json(json!({ "path": root.join("notes.md").display().to_string() }))
        .await
        .unwrap_err();

    assert_eq!(content, "Hello wolf");
    assert!(escaped.to_string().contains("outside"), "{escaped}");
    assert!(absolute.to_string().contains("relative"), "{absolute}");
    assert_eq!(read.descriptor().parameters["required"], json!(["path"]));
}

#[cfg(unix)]
#[tokio::test]
async fn refuses_symlinks_that_lead_out_of_the_root() {
    let root = root("symlink");
    std::fs::write(root.join("../outside.txt"), "outside").unwrap();
    std::os::unix::fs::symlink(root.join("../outside.txt"), root.join("link.txt")).unwrap();
    std::os::unix::fs::symlink(root.join(".."), root.join("up")).unwrap();
    let tools = FileTools::new(&root).unwrap().tools(["read_file"]).unwrap();
    let read = tool(&tools, "read_file");

    let file = read
        .call_json(json!({ "path": "link.txt" }))
        .await
        .unwrap_err();
    // Targets that do not exist (yet) must not lie behind a symlink either.
    let missing = read
        .call_json(json!({ "path": "up/neu/datei.txt" }))
        .await
        .unwrap_err();

    assert!(file.to_string().contains("outside"), "{file}");
    assert!(missing.to_string().contains("outside"), "{missing}");
}

#[tokio::test]
async fn lists_directories_sorted_with_a_slash_after_subdirectories() {
    let root = root("list");
    std::fs::write(root.join("docs/adr.md"), "ADR").unwrap();
    let tools = FileTools::new(&root)
        .unwrap()
        .tools(["list_directory"])
        .unwrap();
    let list = tool(&tools, "list_directory");

    let top = list.call_json(json!({ "path": "." })).await.unwrap();
    let docs = list.call_json(json!({ "path": "docs" })).await.unwrap();

    assert_eq!(top, "docs/\nnotes.md");
    assert_eq!(docs, "adr.md");
}

#[tokio::test]
async fn writes_new_files_and_edits_unique_passages() {
    let root = root("write");
    let tools = FileTools::new(&root)
        .unwrap()
        .tools(["write_file", "edit_file"])
        .unwrap();
    let (write, edit) = (tool(&tools, "write_file"), tool(&tools, "edit_file"));

    let written = write
        .call_json(json!({ "path": "out/plan.md", "content": "Step 1\nStep 2\nStep 2" }))
        .await
        .unwrap();
    let edited = edit
        .call_json(json!({ "path": "out/plan.md", "old": "Step 1", "new": "Start" }))
        .await
        .unwrap();
    let ambiguous = edit
        .call_json(json!({ "path": "out/plan.md", "old": "Step 2", "new": "End" }))
        .await
        .unwrap_err();
    let absent = edit
        .call_json(json!({ "path": "out/plan.md", "old": "Step 9", "new": "x" }))
        .await
        .unwrap_err();

    assert_eq!(written, "out/plan.md written (20 bytes)");
    assert_eq!(edited, "out/plan.md changed");
    assert_eq!(
        std::fs::read_to_string(root.join("out/plan.md")).unwrap(),
        "Start\nStep 2\nStep 2"
    );
    assert!(ambiguous.to_string().contains("2 times"), "{ambiguous}");
    assert!(absent.to_string().contains("not found"), "{absent}");
}

/// Regex search (addendum to ADR-0046): matches as `path:line: text`, sorted, without hidden
/// directories; optionally restricted to a subdirectory.
#[tokio::test]
async fn searches_files_below_the_root_with_a_regular_expression() {
    let root = root("search");
    std::fs::write(root.join("docs/a.md"), "one\nWolf two\n").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/b.rs"), "fn wolf() {}\n").unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/config"), "Wolf\n").unwrap();
    let tools = FileTools::new(&root)
        .unwrap()
        .tools(["search_files"])
        .unwrap();
    let search = tool(&tools, "search_files");

    let everywhere = search
        .call_json(json!({ "pattern": "(?i)wolf" }))
        .await
        .unwrap();
    let in_docs = search
        .call_json(json!({ "pattern": "Wolf", "path": "docs" }))
        .await
        .unwrap();
    let invalid = search
        .call_json(json!({ "pattern": "(unclosed" }))
        .await
        .unwrap_err();
    let escaped = search
        .call_json(json!({ "pattern": "x", "path": ".." }))
        .await
        .unwrap_err();

    assert_eq!(
        everywhere,
        "docs/a.md:2: Wolf two\nnotes.md:1: Hello wolf\nsrc/b.rs:1: fn wolf() {}"
    );
    assert_eq!(in_docs, "docs/a.md:2: Wolf two");
    assert!(invalid.to_string().contains("pattern"), "{invalid}");
    assert!(escaped.to_string().contains("outside"), "{escaped}");
}
