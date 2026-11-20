//! Runs an empty, upload-capable in-memory test registry on 127.0.0.1 until it is killed; for
//! live checks of `grauwolf tools push` without a real registry (ADR-0107).
//!
//! `cargo run -p grauwolf-oci --features test-registry --example serve_empty_registry`
//!
//! Prints `registry: 127.0.0.1:<port>`; then e.g.
//! `grauwolf tools push <folder>/dist 127.0.0.1:<port>/team/add:1.0`.

use grauwolf_oci::testing::TestRegistry;

#[tokio::main]
async fn main() {
    let registry = TestRegistry::start().await;
    println!("registry: {}", registry.host());
    std::future::pending::<()>().await;
}
