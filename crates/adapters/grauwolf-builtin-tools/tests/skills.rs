//! Skill tools (ADR-0103).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use grauwolf_builtin_tools::{MAX_SKILL_FILE_BYTES, SkillLibrary, SkillRoot};
use grauwolf_tools::DynTool;
use serde_json::json;

fn dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("skills-tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn skill(root: &Path, name: &str, description: &str, body: &str) {
    let folder = root.join(name);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n{body}"),
    )
    .unwrap();
}

fn root(dir: &Path) -> SkillRoot {
    SkillRoot {
        dir: dir.to_owned(),
        names: None,
    }
}

fn load(dir: &Path) -> Result<SkillLibrary, String> {
    SkillLibrary::load(&[root(dir)])
}

fn tool(library: &SkillLibrary, name: &str) -> Arc<dyn DynTool> {
    library
        .tools()
        .into_iter()
        .find(|tool| tool.name() == name)
        .unwrap()
}

#[tokio::test]
async fn load_skill_returns_the_instructions_without_front_matter_and_lists_the_files() {
    let dir = dir("load");
    skill(&dir, "haiku-writer", "Writes haiku", "Answer in 5-7-5.\n");
    std::fs::create_dir_all(dir.join("haiku-writer/refs")).unwrap();
    std::fs::write(dir.join("haiku-writer/refs/style.md"), "Nature.").unwrap();
    let library = load(&dir).unwrap();
    let text = tool(&library, "load_skill")
        .call_json(json!({ "name": "haiku-writer" }))
        .await
        .unwrap();
    assert!(text.starts_with("Answer in 5-7-5."), "{text}");
    assert!(!text.contains("description:"));
    assert!(text.contains("- refs/style.md"), "{text}");
    assert!(!text.contains("- SKILL.md"));
}

#[tokio::test]
async fn an_unknown_skill_lists_the_known_names() {
    let dir = dir("unknown");
    skill(&dir, "alpha", "A", "a");
    skill(&dir, "beta", "B", "b");
    let library = load(&dir).unwrap();
    let error = tool(&library, "load_skill")
        .call_json(json!({ "name": "gamma" }))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("gamma") && error.contains("alpha, beta"),
        "{error}"
    );
}

#[tokio::test]
async fn read_skill_file_reads_text_and_refuses_escapes_binary_and_large_files() {
    let dir = dir("read");
    skill(&dir, "s", "S", "x");
    std::fs::write(dir.join("s/note.txt"), "hello").unwrap();
    std::fs::write(dir.join("secret.txt"), "secret").unwrap();
    std::fs::write(dir.join("s/blob.bin"), [0xff, 0xfe, 0x00]).unwrap();
    let library = load(&dir).unwrap();
    let read = tool(&library, "read_skill_file");
    let call = |path: &str| read.call_json(json!({ "name": "s", "path": path }));
    assert_eq!(call("note.txt").await.unwrap(), "hello");
    for escape in ["../secret.txt", "/etc/passwd", ""] {
        let error = call(escape).await.unwrap_err().to_string();
        assert!(error.contains("relative"), "{escape}: {error}");
    }
    assert!(
        call("blob.bin")
            .await
            .unwrap_err()
            .to_string()
            .contains("binary")
    );
    assert!(call("missing.txt").await.is_err());
    // A file that grows after loading is refused as well.
    std::fs::write(
        dir.join("s/big.txt"),
        vec![b'a'; MAX_SKILL_FILE_BYTES as usize + 1],
    )
    .unwrap();
    let error = call("big.txt").await.unwrap_err().to_string();
    assert!(error.contains("larger"), "{error}");
}

#[cfg(unix)]
#[test]
fn a_symlink_that_leaves_the_skill_folder_is_refused_when_loading() {
    let dir = dir("symlink");
    skill(&dir, "s", "S", "x");
    std::fs::write(dir.join("outside.txt"), "o").unwrap();
    std::os::unix::fs::symlink("../outside.txt", dir.join("s/link.txt")).unwrap();
    let error = load(&dir).unwrap_err();
    assert!(error.contains("symlink"), "{error}");
}

#[test]
fn loading_checks_front_matter_names_sizes_and_duplicates() {
    let dir = dir("checks");
    skill(&dir, "ok", "fine", "x");
    assert_eq!(load(&dir).unwrap().list(), vec![("ok", "fine")]);

    let bad = |name: &str, content: &str| {
        let dir = dir.join(name);
        std::fs::create_dir_all(dir.join("s")).unwrap();
        std::fs::write(dir.join("s/SKILL.md"), content).unwrap();
        load(&dir).unwrap_err()
    };
    assert!(bad("nofront", "just text").contains("front matter"));
    assert!(bad("nodesc", "---\nname: s\n---\nx").contains("`description`"));
    assert!(bad("noname", "---\ndescription: d\n---\nx").contains("`name`"));
    assert!(bad("other", "---\nname: t\ndescription: d\n---\nx").contains("differs"));

    let big = dir.join("big");
    skill(&big, "s", "S", "x");
    std::fs::write(
        big.join("s/f.txt"),
        vec![b'a'; MAX_SKILL_FILE_BYTES as usize + 1],
    )
    .unwrap();
    assert!(load(&big).unwrap_err().contains("larger"));

    let first = dir.join("first");
    let second = dir.join("second");
    skill(&first, "same", "A", "x");
    skill(&second, "same", "B", "y");
    let error = SkillLibrary::load(&[root(&first), root(&second)]).unwrap_err();
    assert!(error.contains("more than once"), "{error}");
}

#[test]
fn names_select_skills_and_a_missing_one_is_an_error() {
    let dir = dir("names");
    skill(&dir, "a", "A", "x");
    skill(&dir, "b", "B", "y");
    let only_b = SkillRoot {
        dir: dir.clone(),
        names: Some(vec!["b".into()]),
    };
    assert_eq!(
        SkillLibrary::load(&[only_b]).unwrap().list(),
        vec![("b", "B")]
    );
    let missing = SkillRoot {
        dir,
        names: Some(vec!["c".into()]),
    };
    assert!(
        SkillLibrary::load(&[missing])
            .unwrap_err()
            .contains("not found")
    );
}

#[test]
fn the_prompt_block_lists_skills_and_the_load_skill_tool_carries_it() {
    let dir = dir("block");
    skill(&dir, "a", "does A", "x");
    skill(&dir, "b", "does B", "y");
    let library = load(&dir).unwrap();
    let block = library.prompt_block();
    assert!(block.contains("- a: does A\n- b: does B\n"), "{block}");
    assert!(block.contains("`load_skill`"));
    assert_eq!(tool(&library, "load_skill").prompt_addition(), Some(block));
    assert_eq!(tool(&library, "read_skill_file").prompt_addition(), None);
}
