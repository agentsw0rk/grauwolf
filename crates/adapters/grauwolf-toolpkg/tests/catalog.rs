//! The tool catalog in the registry (ADR-0124): reading and conditional writing with retries,
//! entries made from packages, the index signature and the reader's policy.

use std::time::Duration;

use grauwolf_oci::cosign::SigningKey;
use grauwolf_spec::OciReference;
use grauwolf_toolpkg::catalog::{
    CatalogStore, IndexSignature, MAX_ATTEMPTS, catalog_entry, catalog_reference, entry_verdict,
};
use grauwolf_toolpkg::testing::{PONG_MODULE, TestPackage, TestRegistry};
use grauwolf_toolpkg::{Policy, RegistryClient};
use grauwolf_tools::ToolPolicy;
use grauwolf_tools::catalog::{CatalogEntry, CatalogIndex};
use serde_json::json;

fn package(name: &str, version: &str) -> TestPackage {
    TestPackage::new(name, version)
        .tool(
            "ping",
            "Answers pong",
            json!({ "type": "object" }),
            PONG_MODULE.as_bytes(),
        )
        .limits("ping", json!({ "fuel": 1000 }))
        .annotation("org.opencontainers.image.description", "Pings")
        .annotation("org.opencontainers.image.created", "2026-11-16T09:30:00Z")
        .annotation("org.opencontainers.image.authors", "Ada, Bob")
}

fn reference(registry: &TestRegistry, path: &str) -> OciReference {
    OciReference::parse(&format!("{}/{path}", registry.host())).unwrap()
}

fn now() -> String {
    "2026-11-16T10:00:00Z".to_owned()
}

async fn entry(registry: &TestRegistry, path: &str) -> CatalogEntry {
    catalog_entry(&RegistryClient::default(), &reference(registry, path), &[])
        .await
        .unwrap()
}

/// How an entry is recognised as ours: it is in the index with its digest.
fn holds(entry: &CatalogEntry) -> impl Fn(&CatalogIndex) -> bool {
    let entry = entry.clone();
    move |index| index.entries.get(&entry.key()) == Some(&entry)
}

fn store<'a>(client: &'a RegistryClient, reference: OciReference) -> CatalogStore<'a> {
    CatalogStore::new(client, reference).with_settle(Duration::ZERO, Duration::ZERO)
}

/// An update that adds `entry`.
fn add(entry: CatalogEntry) -> impl FnMut(&mut CatalogIndex) -> Result<bool, String> {
    move |index| Ok(index.upsert(entry.clone()))
}

#[test]
fn a_bare_registry_names_the_default_catalog() {
    let reference = catalog_reference("registry.example.com").unwrap();
    assert_eq!(reference.repository, "grauwolf-catalog");
    assert_eq!(reference.reference(), "latest");
    let custom = catalog_reference("localhost:5000/team/tools-index:v1").unwrap();
    assert_eq!(
        (custom.registry.as_str(), custom.repository.as_str()),
        ("localhost:5000", "team/tools-index")
    );
    assert!(catalog_reference(&format!("r.example.com/c@sha256:{}", "0".repeat(64))).is_err());
}

#[tokio::test]
async fn an_entry_carries_the_manifest_annotations_tools_and_signatures() {
    let registry = TestRegistry::start().await;
    let digest = registry.push("acme/ping", "1.0", &package("ping-pack", "1.0.0"));
    let key = SigningKey::generate();
    let client = RegistryClient::default();
    client
        .push_signature(&reference(&registry, "acme/ping"), &digest, &key)
        .await
        .unwrap();

    let entry = catalog_entry(
        &client,
        &reference(&registry, "acme/ping:1.0"),
        &[key.public_key()],
    )
    .await
    .unwrap();

    assert_eq!(entry.key(), "ping-pack@1.0.0");
    assert_eq!(
        entry.reference,
        format!("{}/acme/ping@{digest}", registry.host())
    );
    assert_eq!(entry.description.as_deref(), Some("Pings"));
    assert_eq!(entry.created.as_deref(), Some("2026-11-16T09:30:00Z"));
    assert_eq!(entry.authors, ["Ada", "Bob"]);
    assert_eq!(entry.tools[0].size, PONG_MODULE.len() as u64);
    assert_eq!(entry.tools[0].limits.fuel, Some(1000));
    assert!(entry.signature.signed);
    assert_eq!(
        entry.signature.fingerprints,
        [key.public_key().fingerprint()]
    );
}

#[tokio::test]
async fn the_first_update_creates_the_index_and_a_second_one_keeps_it() {
    let registry = TestRegistry::start().await;
    registry.push("acme/ping", "1.0", &package("ping-pack", "1.0.0"));
    let client = RegistryClient::default();
    let store = store(&client, catalog_reference(&registry.host()).unwrap());
    assert!(store.read().await.unwrap().digest.is_none());
    let entry = entry(&registry, "acme/ping:1.0").await;

    let first = store
        .update(&mut add(entry.clone()), &holds(&entry), &now, None)
        .await
        .unwrap();
    let again = store
        .update(&mut add(entry.clone()), &holds(&entry), &now, None)
        .await
        .unwrap();

    assert!(first.written && !again.written);
    assert_eq!(again.digest, first.digest);
    let read = store.read().await.unwrap();
    assert_eq!(read.digest, first.digest);
    assert_eq!(read.index.updated, now());
    assert_eq!(read.index.list()[0].key(), "ping-pack@1.0.0");
}

#[tokio::test]
async fn a_concurrent_write_after_an_unconditional_one_is_noticed_and_both_changes_survive() {
    let registry = TestRegistry::start().await;
    registry.push("acme/a", "1", &package("a", "1"));
    registry.push("acme/b", "1", &package("b", "1"));
    let client = RegistryClient::default();
    let catalog = catalog_reference(&registry.host()).unwrap();
    let store = store(&client, catalog.clone());
    let a = entry(&registry, "acme/a:1").await;
    store
        .update(&mut add(a.clone()), &holds(&a), &now, None)
        .await
        .unwrap();
    // The other writer's index: a and another entry, as if it read before us.
    let mut other = store.read().await.unwrap().index;
    let mut c = entry(&registry, "acme/a:1").await;
    c.name = "c".into();
    other.upsert(c);
    let other_manifest = write_aside(&registry, &other).await;
    registry.interfere_with_next_put("grauwolf-catalog", "latest", &other_manifest);

    let b = entry(&registry, "acme/b:1").await;
    let updated = store
        .update(&mut add(b.clone()), &holds(&b), &now, None)
        .await
        .unwrap();

    assert_eq!(updated.attempts, 2);
    assert!(updated.ignores_conditions);
    let keys: Vec<String> = store
        .read()
        .await
        .unwrap()
        .index
        .list()
        .iter()
        .map(|e| e.key())
        .collect();
    assert_eq!(keys, ["a@1", "b@1", "c@1"]);
}

#[tokio::test]
async fn a_conditional_write_that_lost_the_race_is_retried() {
    let registry = TestRegistry::start().await;
    registry.honour_conditional_writes();
    registry.push("acme/a", "1", &package("a", "1"));
    let client = RegistryClient::default();
    let store = store(&client, catalog_reference(&registry.host()).unwrap());
    let mut other = CatalogIndex::new(now());
    let mut c = entry(&registry, "acme/a:1").await;
    c.name = "c".into();
    other.upsert(c);
    let other_manifest = write_aside(&registry, &other).await;
    registry.interfere_with_next_put("grauwolf-catalog", "latest", &other_manifest);

    let a = entry(&registry, "acme/a:1").await;
    let updated = store
        .update(&mut add(a.clone()), &holds(&a), &now, None)
        .await
        .unwrap();

    assert_eq!(updated.attempts, 2);
    assert!(!updated.ignores_conditions);
    assert_eq!(store.read().await.unwrap().index.entries.len(), 2);
}

#[tokio::test]
async fn an_index_that_keeps_changing_is_given_up_after_a_bound() {
    let registry = TestRegistry::start().await;
    registry.honour_conditional_writes();
    registry.push("acme/a", "1", &package("a", "1"));
    let client = RegistryClient::default();
    let store = store(&client, catalog_reference(&registry.host()).unwrap());
    // A different index of another writer for every attempt.
    let mut others = Vec::new();
    for n in 0..MAX_ATTEMPTS {
        let mut other = CatalogIndex::new(now());
        let mut c = entry(&registry, "acme/a:1").await;
        c.name = format!("c{n}");
        other.upsert(c);
        others.push(write_aside(&registry, &other).await);
    }
    let a = entry(&registry, "acme/a:1").await;
    let mut attempts = 0;
    let mut change = |index: &mut CatalogIndex| {
        // Another writer gets in before every write.
        registry.interfere_with_next_put("grauwolf-catalog", "latest", &others[attempts]);
        attempts += 1;
        Ok(index.upsert(a.clone()))
    };

    let error = store
        .update(&mut change, &holds(&a), &now, None)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("kept changing"), "{error}");
    assert_eq!(attempts, MAX_ATTEMPTS);
}

#[tokio::test]
async fn a_signed_index_reports_its_key() {
    let registry = TestRegistry::start().await;
    registry.push("acme/a", "1", &package("a", "1"));
    let client = RegistryClient::default();
    let store = store(&client, catalog_reference(&registry.host()).unwrap());
    let key = SigningKey::generate();
    let a = entry(&registry, "acme/a:1").await;
    let updated = store
        .update(&mut add(a.clone()), &holds(&a), &now, Some(&key))
        .await
        .unwrap();
    let digest = updated.digest.unwrap();

    assert_eq!(
        store.signature(&digest, &[key.public_key()]).await.unwrap(),
        IndexSignature::Valid {
            fingerprint: key.public_key().fingerprint()
        }
    );
    let stranger = SigningKey::generate().public_key();
    assert!(matches!(
        store.signature(&digest, &[stranger]).await.unwrap(),
        IndexSignature::Invalid(_)
    ));
    assert_eq!(
        store.signature(&digest, &[]).await.unwrap(),
        IndexSignature::Unchecked { signatures: 1 }
    );
    let unsigned = store
        .update(
            &mut |index: &mut CatalogIndex| Ok(index.remove("a", "1").is_some()),
            &|index: &CatalogIndex| !index.entries.contains_key("a@1"),
            &now,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .signature(&unsigned.digest.unwrap(), &[key.public_key()])
            .await
            .unwrap(),
        IndexSignature::Unsigned
    );
}

#[tokio::test]
async fn the_readers_policy_decides_per_entry() {
    let registry = TestRegistry::start().await;
    let digest = registry.push("acme/a", "1", &package("a", "1"));
    let key = SigningKey::generate();
    let client = RegistryClient::default();
    client
        .push_signature(&reference(&registry, "acme/a"), &digest, &key)
        .await
        .unwrap();
    let entry = entry(&registry, "acme/a:1").await;
    let policy = |text: &str, keys| Policy::new(ToolPolicy::parse(text).unwrap(), keys);

    assert_eq!(entry_verdict(None, &entry), Ok(()));
    let elsewhere = policy(
        "[tool_packages]\nallowed_registries = [\"ghcr.io\"]\n",
        vec![],
    );
    assert!(
        entry_verdict(Some(&elsewhere), &entry)
            .unwrap_err()
            .contains("allowed_registries")
    );
    let signed = "[tool_packages]\nrequire_signatures = true\noci_keys = [\"k.pub\"]\n";
    assert_eq!(
        entry_verdict(Some(&policy(signed, vec![key.public_key()])), &entry),
        Ok(())
    );
    let other = policy(signed, vec![SigningKey::generate().public_key()]);
    assert!(
        entry_verdict(Some(&other), &entry)
            .unwrap_err()
            .contains("signature")
    );
}

/// Stores an index as another writer would have written it and returns its manifest.
async fn write_aside(registry: &TestRegistry, index: &CatalogIndex) -> Vec<u8> {
    let client = RegistryClient::default();
    let aside = OciReference::parse(&format!("{}/aside/catalog:x", registry.host())).unwrap();
    let store = store(&client, aside);
    let index = index.clone();
    store
        .update(
            &mut |current: &mut CatalogIndex| {
                *current = index.clone();
                Ok(true)
            },
            &|_: &CatalogIndex| true,
            &now,
            None,
        )
        .await
        .unwrap();
    registry.manifest("aside/catalog", "x").unwrap()
}

#[tokio::test]
async fn a_changed_index_that_still_holds_our_entry_is_accepted() {
    let registry = TestRegistry::start().await;
    registry.push("acme/a", "1", &package("a", "1"));
    registry.push("acme/b", "1", &package("b", "1"));
    let client = RegistryClient::default();
    let store = store(&client, catalog_reference(&registry.host()).unwrap());
    let a = entry(&registry, "acme/a:1").await;
    let b = entry(&registry, "acme/b:1").await;
    // Another writer's index right after ours: it has our entry and one more.
    let mut other = CatalogIndex::new(now());
    other.upsert(a.clone());
    other.upsert(b.clone());
    let mut c = a.clone();
    c.name = "c".into();
    other.upsert(c);
    let other_manifest = write_aside(&registry, &other).await;
    registry.interfere_with_next_put("grauwolf-catalog", "latest", &other_manifest);

    let updated = store
        .update(&mut add(b.clone()), &holds(&b), &now, None)
        .await
        .unwrap();

    assert_eq!(
        updated.attempts, 1,
        "the whole-index digest is not compared"
    );
    assert_eq!(store.read().await.unwrap().index.entries.len(), 3);
}

#[tokio::test]
async fn a_removal_that_was_overwritten_is_done_again() {
    let registry = TestRegistry::start().await;
    registry.push("acme/a", "1", &package("a", "1"));
    let client = RegistryClient::default();
    let store = store(&client, catalog_reference(&registry.host()).unwrap());
    let a = entry(&registry, "acme/a:1").await;
    let mut b = a.clone();
    b.name = "b".into();
    let mut both = CatalogIndex::new(now());
    both.upsert(a.clone());
    both.upsert(b.clone());
    let both_manifest = write_aside(&registry, &both).await;
    store
        .update(&mut add(a.clone()), &holds(&a), &now, None)
        .await
        .unwrap();
    // The other writer's index still has `a`.
    registry.interfere_with_next_put("grauwolf-catalog", "latest", &both_manifest);

    let updated = store
        .update(
            &mut |index: &mut CatalogIndex| Ok(index.remove("a", "1").is_some()),
            &|index: &CatalogIndex| !index.entries.contains_key("a@1"),
            &now,
            None,
        )
        .await
        .unwrap();

    assert_eq!(updated.attempts, 2);
    let keys: Vec<String> = store
        .read()
        .await
        .unwrap()
        .index
        .entries
        .keys()
        .cloned()
        .collect();
    assert_eq!(keys, ["b@1"]);
}

#[tokio::test]
async fn a_write_on_a_registry_that_ignores_conditions_is_checked_after_a_wait() {
    let registry = TestRegistry::start().await;
    registry.push("acme/a", "1", &package("a", "1"));
    let client = RegistryClient::default();
    let store = CatalogStore::new(&client, catalog_reference(&registry.host()).unwrap())
        .with_settle(Duration::from_millis(80), Duration::from_millis(80));
    let a = entry(&registry, "acme/a:1").await;

    let started = std::time::Instant::now();
    let updated = store
        .update(&mut add(a.clone()), &holds(&a), &now, None)
        .await
        .unwrap();

    assert!(updated.ignores_conditions);
    assert!(started.elapsed() >= Duration::from_millis(80));
}

#[tokio::test]
async fn a_registry_that_honours_conditions_needs_no_wait() {
    let registry = TestRegistry::start().await;
    registry.honour_conditional_writes();
    registry.push("acme/a", "1", &package("a", "1"));
    let client = RegistryClient::default();
    let store = CatalogStore::new(&client, catalog_reference(&registry.host()).unwrap())
        .with_settle(Duration::from_secs(30), Duration::from_secs(30));
    let a = entry(&registry, "acme/a:1").await;

    let updated = store
        .update(&mut add(a.clone()), &holds(&a), &now, None)
        .await
        .unwrap();

    assert!(updated.written && !updated.ignores_conditions);
}
