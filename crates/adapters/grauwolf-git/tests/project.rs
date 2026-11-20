//! The project repository (ADR-0130): clone, commit one folder, push.

use grauwolf_git::{build_source, commit_folder, ensure_project_clone, push};
use grauwolf_test::TestDir;

/// A bare repository to be used as `origin`.
fn bare_remote(dir: &TestDir) -> String {
    let path = dir.join("remote.git");
    git2::Repository::init_bare(&path).unwrap();
    format!("file://{}", path.display())
}

#[test]
fn an_empty_remote_is_cloned_into_a_missing_directory() {
    let root = TestDir::new("project");
    let url = bare_remote(&root);
    let dir = root.join("build");

    let clone = ensure_project_clone(&dir, &url, "main", None).unwrap();

    assert_eq!(clone.branch, "main");
    let repository = git2::Repository::open(&dir).unwrap();
    assert_eq!(
        repository.find_remote("origin").unwrap().url(),
        Ok(url.as_str())
    );
}

/// Commits `files` (path, content) on `branch` of the bare remote through a scratch clone.
fn seed(root: &TestDir, url: &str, branch: &str, files: &[(&str, &str)]) {
    let scratch = root.join(format!("scratch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = git2::Repository::init(&scratch).unwrap();
    repository.remote("origin", url).unwrap();
    if let Ok(mut remote) = repository.find_remote("origin") {
        let _ = remote.fetch(
            &[format!("+refs/heads/{branch}:refs/remotes/origin/{branch}")],
            None,
            None,
        );
    }
    let parent = repository
        .refname_to_id(&format!("refs/remotes/origin/{branch}"))
        .ok()
        .map(|id| repository.find_commit(id).unwrap());
    let mut index = repository.index().unwrap();
    for (path, content) in files {
        let full = scratch.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, content).unwrap();
        index.add_path(std::path::Path::new(path)).unwrap();
    }
    let tree = repository.find_tree(index.write_tree().unwrap()).unwrap();
    let signature = git2::Signature::now("seed", "seed@example.com").unwrap();
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    repository
        .commit(
            Some(&format!("refs/heads/{branch}")),
            &signature,
            &signature,
            "seed",
            &tree,
            &parents,
        )
        .unwrap();
    repository
        .find_remote("origin")
        .unwrap()
        .push(&[format!("refs/heads/{branch}:refs/heads/{branch}")], None)
        .unwrap();
    drop(tree);
    drop(parent);
    drop(repository);
    std::fs::remove_dir_all(&scratch).unwrap();
}

#[test]
fn a_non_empty_remote_is_cloned_on_the_requested_branch() {
    let root = TestDir::new("project");
    let url = bare_remote(&root);
    seed(&root, &url, "main", &[("other/a.txt", "main\n")]);
    seed(&root, &url, "dev", &[("bot/Dockerfile", "FROM scratch\n")]);
    let dir = root.join("build");

    ensure_project_clone(&dir, &url, "dev", None).unwrap();

    assert_eq!(
        std::fs::read_to_string(dir.join("bot/Dockerfile")).unwrap(),
        "FROM scratch\n"
    );
    let repository = git2::Repository::open(&dir).unwrap();
    assert_eq!(repository.head().unwrap().name(), Ok("refs/heads/dev"));
}

#[test]
fn an_existing_clone_of_the_same_remote_is_kept() {
    let root = TestDir::new("project");
    let url = bare_remote(&root);
    seed(&root, &url, "main", &[("bot/Dockerfile", "FROM scratch\n")]);
    let dir = root.join("build");
    ensure_project_clone(&dir, &url, "main", None).unwrap();
    std::fs::write(dir.join("bot/local.txt"), "mine\n").unwrap();

    let clone = ensure_project_clone(&dir, &url, "main", None).unwrap();

    assert_eq!(clone.branch, "main");
    assert!(dir.join("bot/local.txt").exists());
}

#[test]
fn an_existing_clone_of_another_remote_is_refused() {
    let root = TestDir::new("project");
    let url = bare_remote(&root);
    let dir = root.join("build");
    ensure_project_clone(&dir, &url, "main", None).unwrap();

    let other = "https://github.com/acme/other.git";
    let error = ensure_project_clone(&dir, other, "main", None).unwrap_err();

    assert!(error.contains(&url), "{error}");
    assert!(error.contains(other), "{error}");
}

#[test]
fn a_folder_with_files_becomes_a_clone_and_keeps_them() {
    let root = TestDir::new("project");
    let url = bare_remote(&root);
    seed(&root, &url, "main", &[("other/a.txt", "remote\n")]);
    let dir = root.join("build");
    std::fs::create_dir_all(dir.join("bot")).unwrap();
    std::fs::write(dir.join("bot/Dockerfile"), "FROM scratch\n").unwrap();

    ensure_project_clone(&dir, &url, "main", None).unwrap();

    let repository = git2::Repository::open(&dir).unwrap();
    assert_eq!(
        repository.find_remote("origin").unwrap().url(),
        Ok(url.as_str())
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("bot/Dockerfile")).unwrap(),
        "FROM scratch\n"
    );
    // HEAD is on the remote's branch, so that the first commit can be pushed.
    let remote_tip = repository
        .refname_to_id("refs/remotes/origin/main")
        .unwrap();
    assert_eq!(repository.head().unwrap().name(), Ok("refs/heads/main"));
    assert_eq!(repository.head().unwrap().target(), Some(remote_tip));
}

#[test]
fn a_folder_with_files_becomes_a_clone_of_an_empty_remote() {
    let root = TestDir::new("project");
    let url = bare_remote(&root);
    let dir = root.join("build");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();

    ensure_project_clone(&dir, &url, "main", None).unwrap();

    let repository = git2::Repository::open(&dir).unwrap();
    assert_eq!(
        repository.find_remote("origin").unwrap().url(),
        Ok(url.as_str())
    );
    assert!(dir.join("a.txt").exists());
}

fn write(dir: &std::path::Path, path: &str, content: &str) {
    let full = dir.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, content).unwrap();
}

/// The paths in the tree of `commit`.
fn files_of(dir: &std::path::Path, commit: &str) -> Vec<String> {
    let repository = git2::Repository::open(dir).unwrap();
    let commit = repository
        .find_commit(git2::Oid::from_str(commit).unwrap())
        .unwrap();
    let mut paths = Vec::new();
    commit
        .tree()
        .unwrap()
        .walk(git2::TreeWalkMode::PreOrder, |root, entry| {
            if entry.kind() == Some(git2::ObjectType::Blob) {
                paths.push(format!("{root}{}", entry.name().unwrap()));
            }
            git2::TreeWalkResult::Ok
        })
        .unwrap();
    paths
}

#[test]
fn only_the_given_folder_is_committed_on_an_unborn_branch() {
    let root = TestDir::new("project");
    let url = bare_remote(&root);
    let dir = root.join("build");
    ensure_project_clone(&dir, &url, "main", None).unwrap();
    write(&dir, "bot/Dockerfile", "FROM scratch\n");
    write(&dir, "other/a.txt", "not yet\n");

    let id = commit_folder(&dir, "bot", "bot: generated", "grauwolf", "g@example.com")
        .unwrap()
        .expect("a commit");

    assert_eq!(files_of(&dir, &id), ["bot/Dockerfile"]);
    let repository = git2::Repository::open(&dir).unwrap();
    assert_eq!(
        repository
            .refname_to_id("refs/heads/main")
            .unwrap()
            .to_string(),
        id
    );
    assert_eq!(
        repository
            .find_commit(git2::Oid::from_str(&id).unwrap())
            .unwrap()
            .message(),
        Ok("bot: generated")
    );
}

#[test]
fn nothing_is_committed_when_the_folder_did_not_change() {
    let root = TestDir::new("project");
    let url = bare_remote(&root);
    let dir = root.join("build");
    ensure_project_clone(&dir, &url, "main", None).unwrap();
    write(&dir, "bot/Dockerfile", "FROM scratch\n");
    commit_folder(&dir, "bot", "first", "grauwolf", "g@example.com").unwrap();
    write(&dir, "other/a.txt", "changes elsewhere\n");

    let second = commit_folder(&dir, "bot", "second", "grauwolf", "g@example.com").unwrap();

    assert_eq!(second, None);
}

#[test]
fn changes_and_deletions_of_the_folder_are_committed() {
    let root = TestDir::new("project");
    let url = bare_remote(&root);
    let dir = root.join("build");
    ensure_project_clone(&dir, &url, "main", None).unwrap();
    write(&dir, "bot/Dockerfile", "FROM scratch\n");
    write(&dir, "bot/old.txt", "old\n");
    write(&dir, "other/a.txt", "a\n");
    commit_folder(&dir, "bot", "first", "grauwolf", "g@example.com").unwrap();
    commit_folder(&dir, "other", "other", "grauwolf", "g@example.com").unwrap();
    write(&dir, "bot/Dockerfile", "FROM alpine\n");
    write(&dir, "bot/new.txt", "new\n");
    std::fs::remove_file(dir.join("bot/old.txt")).unwrap();
    std::fs::remove_file(dir.join("other/a.txt")).unwrap();

    let id = commit_folder(&dir, "bot", "second", "grauwolf", "g@example.com")
        .unwrap()
        .expect("a commit");

    // `other/a.txt` is deleted in the working tree but that is not part of this commit.
    assert_eq!(
        files_of(&dir, &id),
        ["bot/Dockerfile", "bot/new.txt", "other/a.txt"]
    );
}

#[test]
fn a_push_makes_the_project_buildable() {
    let root = TestDir::new("project");
    let url = bare_remote(&root);
    let dir = root.join("build");
    ensure_project_clone(&dir, &url, "main", None).unwrap();
    write(&dir, "bot/Dockerfile", "FROM scratch\n");
    let id = commit_folder(&dir, "bot", "bot: generated", "grauwolf", "g@example.com")
        .unwrap()
        .expect("a commit");
    assert!(build_source(&dir.join("bot")).is_err(), "not pushed yet");

    push(&dir, "main", None).unwrap();

    let source = build_source(&dir.join("bot")).unwrap();
    assert_eq!(source.commit, id);
    assert_eq!(source.path, "bot");
    let remote = git2::Repository::open_bare(root.join("remote.git")).unwrap();
    assert_eq!(
        remote.refname_to_id("refs/heads/main").unwrap().to_string(),
        id
    );
}

#[test]
fn a_push_is_refused_when_the_remote_moved_on() {
    let root = TestDir::new("project");
    let url = bare_remote(&root);
    let first = root.join("first");
    let second = root.join("second");
    ensure_project_clone(&first, &url, "main", None).unwrap();
    ensure_project_clone(&second, &url, "main", None).unwrap();
    write(&first, "bot/Dockerfile", "FROM scratch\n");
    commit_folder(&first, "bot", "first", "grauwolf", "g@example.com").unwrap();
    push(&first, "main", None).unwrap();
    write(&second, "bot/Dockerfile", "FROM alpine\n");
    commit_folder(&second, "bot", "second", "grauwolf", "g@example.com").unwrap();

    let error = push(&second, "main", None).unwrap_err();

    assert!(!error.is_empty(), "{error}");
    let remote = git2::Repository::open_bare(root.join("remote.git")).unwrap();
    let tip = remote.refname_to_id("refs/heads/main").unwrap();
    assert_eq!(remote.find_commit(tip).unwrap().message(), Ok("first"));
}

/// Clones from the server, commits a file and pushes; the blocking calls run off the runtime.
async fn clone_commit_push(
    url: String,
    dir: std::path::PathBuf,
    credentials: Option<grauwolf_git::Credentials>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        ensure_project_clone(&dir, &url, "main", credentials.as_ref())?;
        std::fs::create_dir_all(dir.join("harness")).unwrap();
        std::fs::write(dir.join("harness/a.txt"), "A").unwrap();
        commit_folder(&dir, "harness", "add a", "dev", "dev@example.com")?;
        push(&dir, "main", credentials.as_ref())
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn a_push_over_http_carries_the_token() {
    let root = TestDir::new("project-http");
    let bare = root.join("remote.git");
    git2::Repository::init_bare(&bare).unwrap();
    let server = grauwolf_git::testing::TestGitServer::start(&bare, Some("the-token")).await;
    let credentials = grauwolf_git::Credentials {
        username: "someone".to_owned(),
        token: "the-token".to_owned(),
    };

    clone_commit_push(server.url("r"), root.join("build"), Some(credentials))
        .await
        .unwrap();

    let repository = git2::Repository::open_bare(&bare).unwrap();
    let tip = repository.refname_to_id("refs/heads/main").unwrap();
    assert_eq!(repository.find_commit(tip).unwrap().message(), Ok("add a"));
    let config = std::fs::read_to_string(root.join("build/.git/config")).unwrap();
    assert!(!config.contains("the-token"));
}

#[tokio::test]
async fn a_push_over_http_without_the_token_is_refused() {
    let root = TestDir::new("project-http-anon");
    let bare = root.join("remote.git");
    git2::Repository::init_bare(&bare).unwrap();
    let server = grauwolf_git::testing::TestGitServer::start(&bare, Some("the-token")).await;

    let error = clone_commit_push(server.url("r"), root.join("build"), None)
        .await
        .unwrap_err();

    assert!(error.contains("401"), "{error}");
    assert!(!error.contains("the-token"));
}
