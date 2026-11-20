//! Grants in OpenBao KV v2: metadata and refresh token at separate paths.

use std::sync::Arc;
use std::time::Duration;

use grauwolf_identity::testing::FakeKv;
use grauwolf_identity::{
    Grant, GrantOwner, GrantRequest, GrantStore, IdentityError, OpenBaoGrantStore,
};
use grauwolf_openbao::StaticToken;

fn grant() -> Grant {
    Grant::new(
        GrantOwner {
            oid: "oid-alice".into(),
            name: Some("Alice".into()),
            upn: Some("alice@example.org".into()),
        },
        GrantRequest {
            agent: "spiffe://d.de/ns/coordinator/sa/coordinator".into(),
            trigger: "spiffe://d.de/ns/schedules/sa/morning".into(),
            purposes: vec!["review".into()],
            ..GrantRequest::default()
        },
        1_800_000_000,
        Duration::from_secs(8 * 3600),
    )
    .unwrap()
}

fn store(kv: &FakeKv) -> OpenBaoGrantStore<FakeKv> {
    OpenBaoGrantStore::new(kv.clone(), Arc::new(StaticToken("s.bao".into())), "kv")
}

#[tokio::test]
async fn metadata_and_refresh_token_live_at_separate_paths() {
    let kv = FakeKv::new();
    let store = store(&kv);
    let mut grant = grant();
    store.save(&grant).await.unwrap();
    store.save_token(&grant.id, "rt-1").await.unwrap();
    let metadata = kv.data(&format!("identity/grants/{}", grant.id)).unwrap();
    assert_eq!(metadata["agent"], grant.agent);
    assert_eq!(metadata["owner"]["oid"], "oid-alice");
    assert!(!metadata.to_string().contains("rt-1"));
    assert_eq!(
        kv.data(&format!("identity/grant-tokens/{}", grant.id))
            .unwrap()["refresh_token"],
        "rt-1"
    );
    assert!(kv.tokens_seen().iter().all(|token| token == "s.bao"));

    assert_eq!(store.get(&grant.id).await.unwrap(), Some(grant.clone()));
    grant.last_used = Some(1_800_000_100);
    store.save(&grant).await.unwrap();
    store.save_token(&grant.id, "rt-2").await.unwrap();
    assert_eq!(
        store.token(&grant.id).await.unwrap().as_deref(),
        Some("rt-2")
    );
    let other = self::grant();
    store.save(&other).await.unwrap();
    let mut ids: Vec<String> = vec![grant.id.clone(), other.id.clone()];
    ids.sort();
    let listed: Vec<String> = store
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|g| g.id)
        .collect();
    assert_eq!(listed, ids);

    assert!(store.delete(&grant.id).await.unwrap());
    assert_eq!(store.get(&grant.id).await.unwrap(), None);
    assert_eq!(store.token(&grant.id).await.unwrap(), None);
    assert!(!store.delete(&grant.id).await.unwrap());
    assert_eq!(kv.paths(), [format!("identity/grants/{}", other.id)]);
}

#[tokio::test]
async fn unknown_or_malformed_ids_are_none_and_an_unreachable_openbao_is_unavailable() {
    let kv = FakeKv::new();
    let store = store(&kv);
    assert_eq!(store.list().await.unwrap(), []);
    assert_eq!(
        store
            .get("grt_0123456789abcdef0123456789abcdef")
            .await
            .unwrap(),
        None
    );
    assert_eq!(store.get("../../sys/policy").await.unwrap(), None);
    assert!(!store.delete("../x").await.unwrap());
    kv.set_down(true);
    let error = store
        .get("grt_0123456789abcdef0123456789abcdef")
        .await
        .unwrap_err();
    assert!(matches!(error, IdentityError::Unavailable(_)), "{error:?}");
}
