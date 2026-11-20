//! Serves one tool package from the in-memory test registry on 127.0.0.1 until it is killed;
//! for live checks without a real registry (ADR-0107).
//!
//! `cargo run -p grauwolf-toolpkg --features test-registry --example serve_test_package -- \
//!     <module.wasm> <tool> <description> <args-schema-json> <repository:tag>`
//!
//! Prints `registry: 127.0.0.1:<port>` once the package is stored, e.g. then
//! `grauwolf tools pull 127.0.0.1:<port>/test/add:1.0`.

use grauwolf_toolpkg::testing::{TestPackage, TestRegistry};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [module, tool, description, schema, target] = &args[..] else {
        eprintln!(
            "usage: serve_test_package <module.wasm> <tool> <description> <args-schema-json> <repository:tag>"
        );
        std::process::exit(2);
    };
    let (repository, tag) = target.split_once(':').unwrap_or((target, "latest"));
    let module = std::fs::read(module).expect("read the module");
    let schema = serde_json::from_str(schema).expect("args schema as JSON");
    let registry = TestRegistry::start().await;
    let digest = registry.push(
        repository,
        tag,
        &TestPackage::new(tool, "1.0.0").tool(tool, description, schema, &module),
    );
    println!("registry: {}", registry.host());
    println!("digest: {digest}");
    std::future::pending::<()>().await;
}
