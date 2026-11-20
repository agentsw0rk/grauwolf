//! Connection errors: clear texts that never contain the URL or the password. Needs no database:
//! the connection is refused.

use grauwolf_store_pgvector::PgVector;
use grauwolf_store_pgvector::postgres::{describe, describe_database};

#[tokio::test]
async fn a_refused_connection_names_the_variable_and_hides_the_password() {
    let error =
        match PgVector::connect("PGVECTOR_URL", "postgres://agent:s3cret@127.0.0.1:1/db").await {
            Ok(_) => panic!("nothing listens on port 1"),
            Err(error) => error.to_string(),
        };

    assert!(error.contains("PGVECTOR_URL"), "{error}");
    assert!(error.contains("cannot connect"), "{error}");
    for secret in ["s3cret", "agent", "127.0.0.1"] {
        assert!(!error.contains(secret), "{error}");
    }
}

#[tokio::test]
async fn a_value_that_is_not_a_url_is_reported_without_repeating_it() {
    let error = match PgVector::connect("PGVECTOR_URL", "hunter2 not a url").await {
        Ok(_) => panic!("not a URL"),
        Err(error) => error.to_string(),
    };

    assert!(error.contains("PGVECTOR_URL"), "{error}");
    assert!(error.contains("not a valid PostgreSQL URL"), "{error}");
    assert!(!error.contains("hunter2"), "{error}");
}

#[test]
fn server_errors_are_explained() {
    let missing = describe_database(
        Some("0A000"),
        "extension \"vector\" is not available",
        "PGVECTOR_URL",
    );
    assert!(
        missing.contains("pgvector extension is not available"),
        "{missing}"
    );

    let denied = describe_database(
        Some("42501"),
        "permission denied to create extension",
        "PGVECTOR_URL",
    );
    assert!(denied.contains("CREATE EXTENSION vector"), "{denied}");

    let login = describe_database(
        Some("28P01"),
        "password authentication failed for user \"x\"",
        "PGVECTOR_URL",
    );
    assert!(
        login.contains("rejected the credentials") && !login.contains("\"x\""),
        "{login}"
    );
}

#[test]
fn other_errors_name_the_variable_too() {
    let text = describe(&sqlx::Error::PoolTimedOut, "MY_URL");
    assert!(text.contains("MY_URL"));
}
