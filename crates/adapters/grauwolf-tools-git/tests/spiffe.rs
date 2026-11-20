//! Git tools with a workload identity (ADR-0110): clone, fetch and push against the test git
//! server in its mTLS mode; who may read and who may push is decided by the server's mapping.

use std::collections::BTreeMap;
use std::sync::Arc;

use git2::Repository;
use grauwolf_git::testing::{SpiffeAccess, TestGitServer, commit_files};
use grauwolf_spiffe::testing::{FakeIdentity, TestCa};
use grauwolf_spiffe::{AuthMode, ClientAuth, SpiffeId, SpiffeMatch};
use grauwolf_test::TestDir;
use grauwolf_tools::DynTool;
use grauwolf_tools_git::{GIT_TOOLS, GitConfig, GitTools, PushPolicy};
use serde_json::{Value, json};

const SERVER: &str = "spiffe://agentswork.de/ns/git/sa/git";
const WRITER: &str = "spiffe://agentswork.de/ns/agents/sa/writer";
const READER: &str = "spiffe://agentswork.de/ns/agents/sa/reader";
const STRANGER: &str = "spiffe://agentswork.de/ns/other/sa/stranger";

const MAPPING: &str = r#"{
  "spiffe://agentswork.de/ns/agents/sa/writer": { "user": "writer", "push": true },
  "spiffe://agentswork.de/ns/agents/*": { "user": "agents", "push": false }
}"#;

struct Setup {
    _dir: TestDir,
    ca: TestCa,
    server: TestGitServer,
    remote: std::path::PathBuf,
    work: std::path::PathBuf,
}

async fn setup(name: &str) -> Setup {
    let dir = TestDir::new(name);
    let remote = dir.join("remote.git");
    let files = BTreeMap::from([("README.md".to_owned(), b"hello".to_vec())]);
    commit_files(&remote, "main", "seed", &files);
    let ca = TestCa::new("agentswork.de");
    let identity = FakeIdentity::with_ca(&ca, SERVER);
    let server = TestGitServer::start_spiffe(
        &remote,
        &identity,
        &["agentswork.de".to_owned()],
        SpiffeAccess::from_json(MAPPING).unwrap(),
    )
    .await;
    let work = dir.join("work");
    Setup {
        _dir: dir,
        ca,
        server,
        remote,
        work,
    }
}

impl Setup {
    fn tools(&self, client: &str, expected: &str) -> Vec<Arc<dyn DynTool>> {
        self.tools_with(client, expected, AuthMode::Mtls, None)
    }

    fn tools_with(
        &self,
        client: &str,
        expected: &str,
        mode: AuthMode,
        audience: Option<&str>,
    ) -> Vec<Arc<dyn DynTool>> {
        let identity = FakeIdentity::with_ca(&self.ca, client);
        let expected = SpiffeMatch::Id(SpiffeId::parse(expected).unwrap());
        let auth = ClientAuth::new(
            Arc::new(identity),
            mode,
            Some(expected),
            audience.map(str::to_owned),
        )
        .unwrap();
        let mut config = GitConfig::new(self.server.url("team/repo"), &self.work);
        config.push_policy = PushPolicy::Any;
        config.auth = Some(Arc::new(auth));
        GitTools::new(config)
            .tools(GIT_TOOLS.iter().copied())
            .unwrap()
    }
}

async fn call(tools: &[Arc<dyn DynTool>], name: &str, args: Value) -> String {
    let tool = tools.iter().find(|tool| tool.name() == name).unwrap();
    tool.call_json(args).await.unwrap()
}

#[tokio::test]
async fn clone_and_push_with_the_right_identity() {
    let setup = setup("git-spiffe-push").await;
    let tools = setup.tools(WRITER, SERVER);

    let cloned = call(&tools, "git_checkout", json!({ "ref": "main" })).await;
    assert!(cloned.starts_with("On branch main"), "{cloned}");
    std::fs::write(setup.work.join("a.txt"), "a").unwrap();
    call(&tools, "git_commit", json!({ "message": "from the agent" })).await;
    let pushed = call(&tools, "git_push", json!({})).await;

    assert_eq!(pushed, "Pushed main");
    let remote = Repository::open_bare(&setup.remote).unwrap();
    let head = remote.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.message().unwrap().trim(), "from the agent");
    let requests = setup.server.requests();
    assert!(
        requests
            .iter()
            .all(|r| r.spiffe_id.as_deref() == Some(WRITER) && r.authorization.is_none()),
        "{requests:?}"
    );
}

#[tokio::test]
async fn a_read_only_identity_can_clone_but_not_push() {
    let setup = setup("git-spiffe-readonly").await;
    let tools = setup.tools(READER, SERVER);

    let cloned = call(&tools, "git_checkout", json!({ "ref": "main" })).await;
    assert!(cloned.starts_with("On branch main"), "{cloned}");
    std::fs::write(setup.work.join("a.txt"), "a").unwrap();
    call(&tools, "git_commit", json!({ "message": "m" })).await;
    let pushed = call(&tools, "git_push", json!({})).await;

    assert!(pushed.contains("403"), "{pushed}");
    let remote = Repository::open_bare(&setup.remote).unwrap();
    assert_eq!(
        remote
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .message()
            .unwrap(),
        "seed"
    );
}

#[tokio::test]
async fn an_unmapped_identity_is_refused_with_403() {
    let setup = setup("git-spiffe-unmapped").await;
    let tools = setup.tools(STRANGER, SERVER);

    let cloned = call(&tools, "git_checkout", json!({ "ref": "main" })).await;

    assert!(cloned.contains("403"), "{cloned}");
    assert!(!setup.work.join("README.md").exists());
}

#[tokio::test]
async fn a_wrong_expected_server_id_fails_with_the_refusal_message() {
    let setup = setup("git-spiffe-wrong-server").await;
    let wrong = "spiffe://agentswork.de/ns/x/sa/x";
    let tools = setup.tools(WRITER, wrong);

    let cloned = call(&tools, "git_checkout", json!({ "ref": "main" })).await;

    assert!(cloned.contains("TLS handshake"), "{cloned}");
    assert!(
        cloned.contains(&format!("the server is {SERVER}, expected {wrong}")),
        "{cloned}"
    );
}

#[tokio::test]
async fn jwt_mode_sends_a_fresh_bearer_token_with_every_call() {
    let setup = setup("git-spiffe-jwt").await;
    let tools = setup.tools_with(WRITER, SERVER, AuthMode::Jwt, Some("git"));

    let cloned = call(&tools, "git_checkout", json!({ "ref": "main" })).await;

    assert!(cloned.starts_with("On branch main"), "{cloned}");
    let requests = setup.server.requests();
    assert!(!requests.is_empty());
    assert!(
        requests.iter().all(|r| r
            .authorization
            .as_deref()
            .is_some_and(|a| a.starts_with("Bearer "))),
        "{requests:?}"
    );
}
