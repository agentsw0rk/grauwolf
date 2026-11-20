//! `fetch_files` against the in-process test server: a bare repository built with libgit2.

use std::collections::BTreeMap;
use std::time::Duration;

use grauwolf_git::testing::{TestGitServer, commit_files, tag};
use grauwolf_git::{FetchRequest, Wanted, fetch_files};

fn files(entries: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
    entries
        .iter()
        .map(|(name, content)| ((*name).to_owned(), content.as_bytes().to_vec()))
        .collect()
}

fn directory(name: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

async fn fetch(
    request: FetchRequest,
    folder: Option<&'static str>,
) -> Result<grauwolf_git::Fetched, grauwolf_git::GitError> {
    tokio::task::spawn_blocking(move || {
        fetch_files(&request, folder, &|path| !path.starts_with("skip"))
    })
    .await
    .unwrap()
}

struct Repo {
    path: std::path::PathBuf,
    first: git2::Oid,
    second: git2::Oid,
}

fn repo(name: &str) -> Repo {
    let path = directory(name);
    let first = commit_files(
        &path,
        "main",
        "one",
        &files(&[
            ("pkg/a.txt", "A1"),
            ("pkg/src/x.rs", "X"),
            ("pkg/skip.txt", "S"),
            ("b.txt", "B"),
        ]),
    );
    tag(&path, "v1", first, true);
    tag(&path, "light", first, false);
    let second = commit_files(
        &path,
        "main",
        "two",
        &files(&[("pkg/a.txt", "A2"), ("b.txt", "B")]),
    );
    commit_files(&path, "dev", "dev", &files(&[("d.txt", "D")]));
    Repo {
        path,
        first,
        second,
    }
}

#[tokio::test]
async fn fetches_a_folder_at_a_full_commit() {
    let repo = repo("full");
    let server = TestGitServer::start(&repo.path, None).await;
    let rev = repo.first.to_string();
    let fetched = fetch(
        FetchRequest::new(server.url("r"), Wanted::Commit(rev.clone())),
        Some("pkg"),
    )
    .await
    .unwrap();
    assert_eq!(fetched.commit, rev);
    assert_eq!(fetched.files, files(&[("a.txt", "A1"), ("src/x.rs", "X")]));
}

#[tokio::test]
async fn fetches_an_abbreviated_commit_of_an_older_state() {
    let repo = repo("short");
    let server = TestGitServer::start(&repo.path, None).await;
    let short = repo.first.to_string()[..8].to_owned();
    let fetched = fetch(
        FetchRequest::new(server.url("r"), Wanted::Commit(short)),
        None,
    )
    .await
    .unwrap();
    assert_eq!(fetched.commit, repo.first.to_string());
    assert_eq!(fetched.files["b.txt"], b"B");
}

#[tokio::test]
async fn resolves_tags_annotated_and_lightweight_and_branches_to_commits() {
    let repo = repo("refs");
    let server = TestGitServer::start(&repo.path, None).await;
    for name in ["v1", "light"] {
        let fetched = fetch(
            FetchRequest::new(server.url("r"), Wanted::Tag(name.into())),
            Some("pkg"),
        )
        .await
        .unwrap();
        assert_eq!(fetched.commit, repo.first.to_string(), "{name}");
        assert_eq!(fetched.files["a.txt"], b"A1");
    }
    let fetched = fetch(
        FetchRequest::new(server.url("r"), Wanted::Branch("main".into())),
        Some("pkg"),
    )
    .await
    .unwrap();
    assert_eq!(fetched.commit, repo.second.to_string());
    assert_eq!(fetched.files["a.txt"], b"A2");
}

#[tokio::test]
async fn names_what_is_missing() {
    let repo = repo("missing");
    let server = TestGitServer::start(&repo.path, None).await;
    let error = fetch(
        FetchRequest::new(server.url("r"), Wanted::Tag("nope".into())),
        None,
    )
    .await
    .unwrap_err();
    assert!(
        error.to_string().contains("nope") || error.to_string().contains("not found"),
        "{error}"
    );
    let error = fetch(
        FetchRequest::new(server.url("r"), Wanted::Commit("0123456789".into())),
        None,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("not found"), "{error}");
    let error = fetch(
        FetchRequest::new(server.url("r"), Wanted::Branch("main".into())),
        Some("nodir"),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("`nodir`"), "{error}");
}

#[tokio::test]
async fn sends_the_token_to_the_server_and_never_shows_it() {
    let repo = repo("token");
    let server = TestGitServer::start(&repo.path, Some("s3cret-token")).await;
    let mut request = FetchRequest::new(server.url("r"), Wanted::Branch("main".into()));
    request.timeout = Duration::from_secs(10);
    let denied = fetch(request.clone(), None).await.unwrap_err();
    assert!(denied.to_string().contains("401"), "{denied}");
    request.token = Some("wrong-token".into());
    let denied = fetch(request.clone(), None).await.unwrap_err();
    assert!(!denied.to_string().contains("wrong-token"), "{denied}");
    request.token = Some("s3cret-token".into());
    assert!(fetch(request, None).await.is_ok());
    assert!(server.requests().iter().any(|r| r.authorization.is_some()));
}

#[tokio::test]
async fn the_token_is_not_sent_without_one() {
    let repo = repo("anonymous");
    let server = TestGitServer::start(&repo.path, None).await;
    fetch(
        FetchRequest::new(server.url("r"), Wanted::Branch("main".into())),
        None,
    )
    .await
    .unwrap();
    assert!(server.requests().iter().all(|r| r.authorization.is_none()));
}
