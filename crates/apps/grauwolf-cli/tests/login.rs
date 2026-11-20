//! `grauwolf login | logout` (ADR-0111): the device code flow against a mocked Entra token
//! endpoint, the token cache (mode 0600), `--status`, `--print-token` with refresh.

use std::process::ExitCode;

use grauwolf_cli::{Captured, Io, run};
use grauwolf_test::TestDir;
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TENANT: &str = "11111111-1111-1111-1111-111111111111";
/// An unsigned JWT for Alice (only its claims are shown, never verified by the CLI).
const ACCESS: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJvaWQiOiIwYTFiIiwibmFtZSI6IkFsaWNlIiwidXBuIjoiYWxpY2VAZW50cmFsb2NhbC5kZXYiLCJ0aWQiOiJ0MSJ9.c2ln";
const ACCESS_2: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJvaWQiOiIwYTFiIiwibmFtZSI6IkFsaWNlIiwicHJlZmVycmVkX3VzZXJuYW1lIjoiYWxpY2VAZW50cmFsb2NhbC5kZXYiLCJ0aWQiOiJ0MSIsIm4iOjJ9.c2ln";

/// An unsigned JWT with an `oid` but no `name`, `upn` or `preferred_username` (Entra Local).
const ACCESS_NO_NAME: &str =
    "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJvaWQiOiIwYTFiIiwidGlkIjoidDEifQ.c2ln";

fn grauwolf(args: &[&str]) -> (ExitCode, Captured) {
    let (io, captured) = Io::captured();
    let code = run(std::iter::once("grauwolf").chain(args.iter().copied()), &io);
    (code, captured)
}

/// A mock server on its own runtime, so that the CLI can block on its own.
fn server() -> (tokio::runtime::Runtime, MockServer) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(MockServer::start());
    (runtime, server)
}

fn token_path() -> String {
    format!("/{TENANT}/oauth2/v2.0/token")
}

fn mount_device_code(runtime: &tokio::runtime::Runtime, server: &MockServer) {
    Mock::given(method("POST"))
        .and(path(format!("/{TENANT}/oauth2/v2.0/devicecode")))
        .and(body_string_contains("client_id=cli-app"))
        .and(body_string_contains("offline_access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code": "dc", "user_code": "BCDF-GHJK",
            "verification_uri": "https://login.example/devicecode",
            "expires_in": 30, "interval": 0})))
        .pipe_mount(runtime, server);
}

trait PipeMount {
    fn pipe_mount(self, runtime: &tokio::runtime::Runtime, server: &MockServer);
}

impl PipeMount for Mock {
    fn pipe_mount(self, runtime: &tokio::runtime::Runtime, server: &MockServer) {
        runtime.block_on(self.mount(server));
    }
}

fn login_args<'a>(server: &'a str, dir: &'a str) -> Vec<&'a str> {
    vec![
        "login",
        "--authority",
        server,
        "--tenant",
        TENANT,
        "--client-id",
        "cli-app",
        "--scope",
        "api://grauwolf-identity/access_as_user",
        "--config-dir",
        dir,
    ]
}

fn cache(dir: &TestDir) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("login.json")).unwrap()).unwrap()
}

#[test]
fn login_shows_the_code_waits_for_the_sign_in_and_caches_the_tokens_privately() {
    let (runtime, server) = server();
    mount_device_code(&runtime, &server);
    Mock::given(method("POST"))
        .and(path(token_path()))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(json!({"error": "authorization_pending"})),
        )
        .up_to_n_times(2)
        .with_priority(1)
        .pipe_mount(&runtime, &server);
    Mock::given(method("POST"))
        .and(path(token_path()))
        .and(body_string_contains("device_code=dc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": ACCESS, "refresh_token": "refresh-1", "expires_in": 3600})))
        .with_priority(2)
        .pipe_mount(&runtime, &server);
    let dir = TestDir::new("login-ok");
    let uri = server.uri();
    let dir_text = dir.path().display().to_string();

    let (code, output) = grauwolf(&login_args(&uri, &dir_text));
    assert_eq!(code, ExitCode::SUCCESS, "{}", output.stderr());
    let out = output.stdout();
    assert!(out.contains("https://login.example/devicecode"), "{out}");
    assert!(out.contains("BCDF-GHJK"), "{out}");
    assert!(
        out.contains("logged in as Alice <alice@entralocal.dev> (oid 0a1b)"),
        "{out}"
    );
    assert!(!out.contains(ACCESS) && !out.contains("refresh-1"), "{out}");

    let cached = cache(&dir);
    assert_eq!(cached["access_token"], ACCESS);
    assert_eq!(cached["refresh_token"], "refresh-1");
    assert_eq!(cached["tenant"], TENANT);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("login.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    let (code, output) = grauwolf(&["login", "--status", "--config-dir", &dir_text]);
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(
        output
            .stdout()
            .contains("logged in as Alice <alice@entralocal.dev>"),
        "{}",
        output.stdout()
    );
    assert!(output.stdout().contains("refresh token: yes"));
    assert!(!output.stdout().contains(ACCESS));

    let (code, output) = grauwolf(&["login", "--print-token", "--config-dir", &dir_text]);
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(output.stdout(), format!("{ACCESS}\n"));
}

#[test]
fn an_expired_device_code_is_an_error_and_nothing_is_cached() {
    let (runtime, server) = server();
    mount_device_code(&runtime, &server);
    Mock::given(method("POST"))
        .and(path(token_path()))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": "expired_token"})))
        .pipe_mount(&runtime, &server);
    let dir = TestDir::new("login-expired");
    let uri = server.uri();
    let dir_text = dir.path().display().to_string();
    let (code, output) = grauwolf(&login_args(&uri, &dir_text));
    assert_eq!(code, ExitCode::from(1));
    assert!(output.stderr().contains("expired"), "{}", output.stderr());
    assert!(!dir.join("login.json").exists());
}

#[test]
fn print_token_refreshes_an_expiring_token_and_keeps_the_cache_up_to_date() {
    let (runtime, server) = server();
    Mock::given(method("POST"))
        .and(path(token_path()))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=refresh-1"))
        .and(body_string_contains("client_id=cli-app"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": ACCESS_2, "refresh_token": "refresh-2", "expires_in": 3600})))
        .expect(1)
        .pipe_mount(&runtime, &server);
    let dir = TestDir::new("login-refresh");
    std::fs::write(
        dir.join("login.json"),
        json!({"authority": server.uri(), "tenant": TENANT, "client_id": "cli-app",
            "scope": "api://grauwolf-identity/access_as_user", "access_token": ACCESS,
            "refresh_token": "refresh-1", "expires_at": 1})
        .to_string(),
    )
    .unwrap();
    let dir_text = dir.path().display().to_string();

    let (code, output) = grauwolf(&["login", "--status", "--config-dir", &dir_text]);
    assert_eq!(code, ExitCode::SUCCESS, "refreshable counts as logged in");
    assert!(
        output.stdout().contains("access token expired"),
        "{}",
        output.stdout()
    );

    let (code, output) = grauwolf(&["login", "--print-token", "--config-dir", &dir_text]);
    assert_eq!(code, ExitCode::SUCCESS, "{}", output.stderr());
    assert_eq!(output.stdout(), format!("{ACCESS_2}\n"));
    let cached = cache(&dir);
    assert_eq!(cached["access_token"], ACCESS_2);
    assert_eq!(cached["refresh_token"], "refresh-2");
    // Valid now: no second refresh (the mock expects exactly one call).
    let (code, output) = grauwolf(&["login", "--print-token", "--config-dir", &dir_text]);
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(output.stdout(), format!("{ACCESS_2}\n"));
    runtime.block_on(server.verify());
}

#[test]
fn an_expired_login_without_refresh_token_and_no_login_at_all_are_errors() {
    let dir = TestDir::new("login-none");
    let dir_text = dir.path().display().to_string();
    let (code, output) = grauwolf(&["login", "--print-token", "--config-dir", &dir_text]);
    assert_eq!(code, ExitCode::from(1));
    assert!(
        output.stderr().contains("not logged in"),
        "{}",
        output.stderr()
    );
    let (code, output) = grauwolf(&["login", "--status", "--config-dir", &dir_text]);
    assert_eq!(code, ExitCode::from(1));
    assert!(output.stdout().contains("not logged in"));

    std::fs::write(
        dir.join("login.json"),
        json!({"authority": "http://127.0.0.1:9", "tenant": TENANT, "client_id": "cli-app",
            "scope": "s", "access_token": ACCESS, "expires_at": 1})
        .to_string(),
    )
    .unwrap();
    let (code, output) = grauwolf(&["login", "--print-token", "--config-dir", &dir_text]);
    assert_eq!(code, ExitCode::from(1));
    assert!(
        output.stderr().contains("no refresh token"),
        "{}",
        output.stderr()
    );
    assert!(!output.stderr().contains(ACCESS));
}

#[test]
fn logout_removes_the_cache() {
    let dir = TestDir::new("logout");
    let dir_text = dir.path().display().to_string();
    std::fs::write(dir.join("login.json"), "{}").unwrap();
    let (code, output) = grauwolf(&["logout", "--config-dir", &dir_text]);
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(output.stdout().contains("logged out"));
    assert!(!dir.join("login.json").exists());
    let (_, output) = grauwolf(&["logout", "--config-dir", &dir_text]);
    assert!(output.stdout().contains("not logged in"));
}

#[test]
fn login_names_what_is_missing() {
    let dir = TestDir::new("login-missing");
    let dir_text = dir.path().display().to_string();
    let (code, output) = grauwolf(&["login", "--tenant", TENANT, "--config-dir", &dir_text]);
    assert_eq!(code, ExitCode::from(1));
    assert!(
        output.stderr().contains("--client-id"),
        "{}",
        output.stderr()
    );
}

#[test]
fn a_token_without_a_name_is_shown_by_its_oid_alone() {
    let (runtime, server) = server();
    mount_device_code(&runtime, &server);
    Mock::given(method("POST"))
        .and(path(token_path()))
        .and(body_string_contains("device_code=dc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": ACCESS_NO_NAME, "refresh_token": "refresh-1", "expires_in": 3600})))
        .pipe_mount(&runtime, &server);
    let dir = TestDir::new("login-no-name");
    let uri = server.uri();
    let dir_text = dir.path().display().to_string();

    let (code, output) = grauwolf(&login_args(&uri, &dir_text));
    assert_eq!(code, ExitCode::SUCCESS, "{}", output.stderr());
    assert!(
        output.stdout().contains("logged in as oid 0a1b at tenant"),
        "{}",
        output.stdout()
    );

    let (_, output) = grauwolf(&["login", "--status", "--config-dir", &dir_text]);
    assert!(
        output.stdout().contains("logged in as oid 0a1b\n"),
        "{}",
        output.stdout()
    );
}
