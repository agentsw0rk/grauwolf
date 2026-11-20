//! SSH signatures of commits and tags (ADR-0107 stage 4): the allowed-signers file and the
//! check of objects fetched from the in-process test server.

use std::collections::BTreeMap;

use grauwolf_git::testing::{
    TestGitServer, TestSigner, commit_files, commit_files_signed, tag, tag_signed,
};
use grauwolf_git::{
    AllowedSigners, FetchRequest, SignatureError, SignedObjects, Wanted, fetch_files,
    verify_objects,
};

fn directory(name: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("sshsig-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn files() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([("a.txt".to_owned(), b"A".to_vec())])
}

/// Fetches `wanted` from the repository at `path` and returns the commit and its objects.
async fn fetched(path: &std::path::Path, wanted: Wanted) -> (String, SignedObjects) {
    let server = TestGitServer::start(path, None).await;
    let request = FetchRequest::new(server.url("r"), wanted);
    let fetched = tokio::task::spawn_blocking(move || fetch_files(&request, None, &|_| false))
        .await
        .unwrap()
        .unwrap();
    (fetched.commit, fetched.objects)
}

fn signers(lines: &[String]) -> AllowedSigners {
    AllowedSigners::parse(&lines.concat()).unwrap()
}

#[tokio::test]
async fn a_commit_signed_by_an_allowed_key_verifies() {
    let path = directory("signed");
    let signer = TestSigner::new();
    commit_files_signed(&path, "main", "one", &files(), &signer);
    let (commit, objects) = fetched(&path, Wanted::Branch("main".into())).await;
    let allowed = signers(&[signer.allowed_signers("*", None)]);
    let verified = verify_objects(&objects, &commit, None, &allowed).unwrap();
    assert_eq!(verified.fingerprint, signer.fingerprint());
    assert_eq!(verified.signed, "commit");
}

#[tokio::test]
async fn an_unsigned_commit_is_unsigned() {
    let path = directory("unsigned");
    commit_files(&path, "main", "one", &files());
    let (commit, objects) = fetched(&path, Wanted::Branch("main".into())).await;
    let allowed = signers(&[TestSigner::new().allowed_signers("*", None)]);
    assert_eq!(
        verify_objects(&objects, &commit, None, &allowed),
        Err(SignatureError::Unsigned)
    );
}

#[tokio::test]
async fn a_commit_signed_by_another_key_is_invalid() {
    let path = directory("wrong");
    commit_files_signed(&path, "main", "one", &files(), &TestSigner::new());
    let (commit, objects) = fetched(&path, Wanted::Branch("main".into())).await;
    let allowed = signers(&[TestSigner::new().allowed_signers("*", None)]);
    let Err(SignatureError::Invalid(reason)) = verify_objects(&objects, &commit, None, &allowed)
    else {
        panic!("expected an invalid signature");
    };
    assert!(reason.contains("not an allowed signer"), "{reason}");
}

#[tokio::test]
async fn principals_and_namespaces_of_the_allowed_signers_file_apply() {
    let path = directory("principals");
    let signer = TestSigner::new();
    commit_files_signed(&path, "main", "one", &files(), &signer);
    let (commit, objects) = fetched(&path, Wanted::Branch("main".into())).await;
    // The committer of the test repository is test@example.com.
    for allowed in [
        "test@example.com",
        "*@example.com",
        "other@x.org,test@example.com",
    ] {
        let allowed = signers(&[signer.allowed_signers(allowed, None)]);
        assert!(verify_objects(&objects, &commit, None, &allowed).is_ok());
    }
    let other = signers(&[signer.allowed_signers("someone@example.com", None)]);
    assert!(verify_objects(&objects, &commit, None, &other).is_err());
    let files_only = signers(&[signer.allowed_signers("*", Some("file"))]);
    assert!(verify_objects(&objects, &commit, None, &files_only).is_err());
    let git_too = signers(&[signer.allowed_signers("*", Some("file,git"))]);
    assert!(verify_objects(&objects, &commit, None, &git_too).is_ok());
}

#[tokio::test]
async fn a_signed_tag_verifies_and_an_unsigned_tag_falls_back_to_the_commit() {
    let path = directory("tags");
    let signer = TestSigner::new();
    let tagger = TestSigner::new();
    let first = commit_files(&path, "main", "one", &files());
    tag_signed(&path, "v1", first, &tagger);
    let second = commit_files_signed(&path, "main", "two", &files(), &signer);
    tag(&path, "v2", second, true);

    let allowed = signers(&[tagger.allowed_signers("*", None)]);
    let (commit, objects) = fetched(&path, Wanted::Tag("v1".into())).await;
    let verified = verify_objects(&objects, &commit, Some("v1"), &allowed).unwrap();
    assert_eq!(
        (verified.signed, verified.fingerprint),
        ("tag", tagger.fingerprint())
    );

    // v2 is an unsigned annotated tag of a signed commit: the commit signature counts.
    let allowed = signers(&[signer.allowed_signers("*", None)]);
    let (commit, objects) = fetched(&path, Wanted::Tag("v2".into())).await;
    let verified = verify_objects(&objects, &commit, Some("v2"), &allowed).unwrap();
    assert_eq!(verified.signed, "commit");

    // The signature of v1's tagger does not make v2 signed.
    let allowed = signers(&[tagger.allowed_signers("*", None)]);
    assert!(verify_objects(&objects, &commit, Some("v2"), &allowed).is_err());
}

#[tokio::test]
async fn objects_of_another_commit_or_tag_are_not_accepted() {
    let path = directory("swapped");
    let signer = TestSigner::new();
    let first = commit_files(&path, "main", "one", &files());
    tag_signed(&path, "v1", first, &signer);
    commit_files(&path, "main", "two", &files());
    let allowed = signers(&[signer.allowed_signers("*", None)]);
    let (commit, objects) = fetched(&path, Wanted::Tag("v1".into())).await;
    // A genuinely signed tag object cannot vouch for another tag name or another commit.
    let error = verify_objects(&objects, &commit, Some("v2"), &allowed).unwrap_err();
    assert!(matches!(error, SignatureError::Invalid(_)), "{error:?}");
    let other = "0".repeat(40);
    assert!(verify_objects(&objects, &other, Some("v1"), &allowed).is_err());
    // A tampered commit object no longer has its id.
    let mut tampered = objects.clone();
    let at = tampered.commit.len() - 2;
    tampered.commit[at] ^= 1;
    assert!(verify_objects(&tampered, &commit, None, &allowed).is_err());
    // A tag whose signed text was changed fails the cryptographic check (not just an id check).
    let mut forged = objects.clone();
    let raw = forged.tag.as_mut().unwrap();
    let at = raw.windows(11).position(|w| w == b"tagger test").unwrap();
    raw[at + 7] = b'T';
    let commit_only = SignedObjects {
        tag: forged.tag,
        ..objects
    };
    let Err(SignatureError::Invalid(reason)) =
        verify_objects(&commit_only, &commit, Some("v1"), &allowed)
    else {
        panic!("a forged tag must not verify");
    };
    // The commit is unsigned, so the forged tag is what is reported.
    assert!(reason.contains("does not match"), "{reason}");
}

#[test]
fn the_allowed_signers_file_is_parsed_like_gits() {
    let signer = TestSigner::new();
    let line = signer.allowed_signers("a@x.org,*@y.org", Some("git"));
    let text = format!("# comment\n\n{line}");
    let parsed = AllowedSigners::parse(&text).unwrap();
    assert_eq!(parsed.fingerprints(), [signer.fingerprint()]);
    assert!(AllowedSigners::parse("").unwrap().is_empty());
}

#[test]
fn unsupported_or_broken_allowed_signers_lines_are_errors_with_the_line_number() {
    let signer = TestSigner::new();
    let key = signer.allowed_signers("*", None).replacen("* ", "", 1);
    for (line, expect) in [
        (format!("* cert-authority {key}"), "cert-authority"),
        (format!("* valid-before=\"20300101\" {key}"), "valid-before"),
        (format!("!bad@x.org {key}"), "negated"),
        (
            "* ssh-ed25519 notbase64".to_owned(),
            "invalid ssh-ed25519 key",
        ),
        ("onlyprincipal".to_owned(), "expected"),
    ] {
        let error = AllowedSigners::parse(&format!("\n{line}")).unwrap_err();
        assert!(error.starts_with("line 2:"), "{error}");
        assert!(error.contains(expect), "{expect} not in {error}");
    }
}
