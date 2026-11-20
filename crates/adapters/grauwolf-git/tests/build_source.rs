//! `build_source`: the commit a harness project is built from in the cluster (ADR-0129).

use grauwolf_git::{BuildSource, build_source};
use grauwolf_test::TestDir;

/// A repository with `origin`, one commit of `build/bot/Dockerfile` on `main`, and `origin/main`
/// at that commit (as after a push).
fn pushed(dir: &TestDir) -> (git2::Repository, git2::Oid) {
    let repository = git2::Repository::init(&**dir).unwrap();
    repository
        .remote("origin", "https://github.com/agentsw0rk/agents.git")
        .unwrap();
    std::fs::create_dir_all(dir.join("build/bot")).unwrap();
    std::fs::write(dir.join("build/bot/Dockerfile"), "FROM scratch\n").unwrap();
    let mut index = repository.index().unwrap();
    index
        .add_path(std::path::Path::new("build/bot/Dockerfile"))
        .unwrap();
    index.write().unwrap();
    let tree = repository.find_tree(index.write_tree().unwrap()).unwrap();
    let signature = git2::Signature::now("t", "t@example.com").unwrap();
    let commit = repository
        .commit(
            Some("refs/heads/main"),
            &signature,
            &signature,
            "first",
            &tree,
            &[],
        )
        .unwrap();
    repository.set_head("refs/heads/main").unwrap();
    repository
        .reference("refs/remotes/origin/main", commit, true, "pushed")
        .unwrap();
    drop(tree);
    (repository, commit)
}

#[test]
fn a_committed_and_pushed_project_folder_names_its_remote_branch_commit_and_path() {
    let dir = TestDir::new("build-source");
    let (_repository, commit) = pushed(&dir);

    assert_eq!(
        build_source(&dir.join("build/bot")),
        Ok(BuildSource {
            remote: "https://github.com/agentsw0rk/agents.git".into(),
            branch: "refs/heads/main".into(),
            commit: commit.to_string(),
            path: "build/bot".into(),
        })
    );
}

#[test]
fn a_project_folder_with_changes_that_are_not_committed_is_refused() {
    let dir = TestDir::new("build-source-dirty");
    let _pushed = pushed(&dir);
    std::fs::write(dir.join("build/bot/Dockerfile"), "FROM scratch\nRUN true\n").unwrap();
    std::fs::write(dir.join("build/bot/new.txt"), "new").unwrap();

    let error = build_source(&dir.join("build/bot")).unwrap_err();

    assert!(
        error.contains(
            "has changes that are not committed (build/bot/Dockerfile, build/bot/new.txt)"
        ),
        "{error}"
    );
}

#[test]
fn a_commit_that_is_not_pushed_is_refused() {
    let dir = TestDir::new("build-source-unpushed");
    let (repository, first) = pushed(&dir);
    std::fs::write(dir.join("build/bot/Dockerfile"), "FROM scratch\nRUN true\n").unwrap();
    let mut index = repository.index().unwrap();
    index
        .add_path(std::path::Path::new("build/bot/Dockerfile"))
        .unwrap();
    index.write().unwrap();
    let tree = repository.find_tree(index.write_tree().unwrap()).unwrap();
    let signature = git2::Signature::now("t", "t@example.com").unwrap();
    let parent = repository.find_commit(first).unwrap();
    repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            "second",
            &tree,
            &[&parent],
        )
        .unwrap();

    let error = build_source(&dir.join("build/bot")).unwrap_err();

    assert!(
        error.contains("is not on origin yet: push refs/heads/main first"),
        "{error}"
    );
}
