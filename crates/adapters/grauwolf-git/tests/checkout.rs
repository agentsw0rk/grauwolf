//! `checkout_of`: the remote and commit of a local checkout (ADR-0124).

use grauwolf_git::checkout_of;
use grauwolf_test::TestDir;

#[test]
fn a_folder_in_a_checkout_names_its_remote_and_commit() {
    let dir = TestDir::new("checkout-of");
    let repository = git2::Repository::init(&*dir).unwrap();
    repository
        .remote("origin", "https://user:secret@example.com/acme/tools.git")
        .unwrap();
    std::fs::create_dir_all(dir.join("pkg")).unwrap();
    std::fs::write(dir.join("pkg/file.txt"), "x").unwrap();
    let mut index = repository.index().unwrap();
    index
        .add_path(std::path::Path::new("pkg/file.txt"))
        .unwrap();
    let tree = repository.find_tree(index.write_tree().unwrap()).unwrap();
    let signature = git2::Signature::now("t", "t@example.com").unwrap();
    let commit = repository
        .commit(Some("HEAD"), &signature, &signature, "first", &tree, &[])
        .unwrap();

    let checkout = checkout_of(&dir.join("pkg")).unwrap();

    assert_eq!(checkout.commit, commit.to_string());
    assert_eq!(
        checkout.remote.as_deref(),
        Some("https://example.com/acme/tools.git")
    );
}

#[test]
fn a_folder_outside_a_repository_has_no_checkout() {
    let dir = TestDir::new("checkout-none");
    assert_eq!(checkout_of(&dir), None);
}
