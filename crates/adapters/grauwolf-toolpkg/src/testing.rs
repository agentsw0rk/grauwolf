//! Test registry for tool packages (feature `test-registry`): the in-memory registry of
//! grauwolf-oci plus [`TestPackage`], which stores a tool package in it.

use std::ops::Deref;

use grauwolf_oci::IMAGE_MANIFEST;
use serde_json::{Value, json};

use crate::{ARTIFACT_TYPE, CONFIG_TYPE, MODULE_TYPE, SOURCE_TYPE, TITLE_ANNOTATION};

/// A module that follows tool ABI v1 and always answers `pong` (WAT text; the sandbox accepts it).
pub const PONG_MODULE: &str = r#"(module
  (memory (export "memory") 1)
  (data (i32.const 1024) "{\"ok\":\"pong\"}")
  (func (export "gw_alloc") (param i32) (result i32) (i32.const 2048))
  (func (export "gw_call") (param i32 i32) (result i64)
    (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const 13))))"#;

/// A package to store in the test registry.
#[derive(Debug, Clone)]
pub struct TestPackage {
    name: String,
    version: String,
    abi: u32,
    tools: Vec<(String, String, Value, Vec<u8>)>,
    source: Option<Vec<u8>>,
    config_type: String,
    annotations: std::collections::BTreeMap<String, String>,
    metadata: serde_json::Map<String, Value>,
    limits: std::collections::BTreeMap<String, Value>,
}

impl TestPackage {
    /// Package without tools yet.
    pub fn new(name: &str, version: &str) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            abi: 1,
            tools: Vec::new(),
            source: None,
            config_type: CONFIG_TYPE.into(),
            annotations: Default::default(),
            metadata: Default::default(),
            limits: Default::default(),
        }
    }

    /// Sets the `limits` of the tool `name` (ADR-0124), e.g. `{ "fuel": 1000 }`.
    pub fn limits(mut self, name: &str, limits: Value) -> Self {
        self.limits.insert(name.into(), limits);
        self
    }

    /// Sets an annotation of the OCI manifest (ADR-0124).
    pub fn annotation(mut self, key: &str, value: &str) -> Self {
        self.annotations.insert(key.into(), value.into());
        self
    }

    /// Sets a top-level field of `grauwolf-tools.json`, e.g. `description` (ADR-0124).
    pub fn field(mut self, key: &str, value: Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Adds a tool whose module is `module` (file `<name>.wasm`).
    pub fn tool(mut self, name: &str, description: &str, args: Value, module: &[u8]) -> Self {
        self.tools
            .push((name.into(), description.into(), args, module.to_vec()));
        self
    }

    /// Declares another tool ABI version.
    pub fn abi(mut self, abi: u32) -> Self {
        self.abi = abi;
        self
    }

    /// Adds a source layer.
    pub fn source(mut self, tarball: &[u8]) -> Self {
        self.source = Some(tarball.to_vec());
        self
    }

    /// Uses another media type for the config blob.
    pub fn config_media_type(mut self, media_type: &str) -> Self {
        self.config_type = media_type.into();
        self
    }
}

/// A running in-memory registry on `127.0.0.1` (stops when dropped) that can store a
/// [`TestPackage`]; everything of the generic registry is available through `Deref`.
pub struct TestRegistry(grauwolf_oci::testing::TestRegistry);

impl Deref for TestRegistry {
    type Target = grauwolf_oci::testing::TestRegistry;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TestRegistry {
    /// Anonymous registry.
    pub async fn start() -> Self {
        Self(grauwolf_oci::testing::TestRegistry::start().await)
    }

    /// Registry that answers `401` with a bearer challenge; see
    /// [`grauwolf_oci::testing::TestRegistry::start_with_token_auth`].
    pub async fn start_with_token_auth(login: Option<(&str, &str)>) -> Self {
        Self(grauwolf_oci::testing::TestRegistry::start_with_token_auth(login).await)
    }

    /// Stores `package` as `repository:tag` and returns the manifest digest.
    pub fn push(&self, repository: &str, tag: &str, package: &TestPackage) -> String {
        let mut layers = Vec::new();
        let tools: Vec<Value> = package
            .tools
            .iter()
            .map(|(name, description, args, _)| {
                let mut tool = json!({ "name": name, "description": description, "args": args,
                        "module": format!("{name}.wasm") });
                if let Some(limits) = package.limits.get(name) {
                    tool["limits"] = limits.clone();
                }
                tool
            })
            .collect();
        let mut config = json!({ "format": 1, "name": package.name, "version": package.version,
                             "abi": package.abi, "tools": tools });
        config
            .as_object_mut()
            .expect("an object")
            .extend(package.metadata.clone());
        let config = config.to_string().into_bytes();
        let config_digest = self.add_blob(&config);
        for (name, _, _, module) in &package.tools {
            let digest = self.add_blob(module);
            layers.push(
                json!({ "mediaType": MODULE_TYPE, "digest": digest, "size": module.len(),
                "annotations": { TITLE_ANNOTATION: format!("{name}.wasm") } }),
            );
        }
        if let Some(source) = &package.source {
            let digest = self.add_blob(source);
            layers.push(
                json!({ "mediaType": SOURCE_TYPE, "digest": digest, "size": source.len(),
                "annotations": { TITLE_ANNOTATION: "src.tar.gz" } }),
            );
        }
        let mut manifest = json!({
            "schemaVersion": 2,
            "mediaType": IMAGE_MANIFEST,
            "artifactType": ARTIFACT_TYPE,
            "config": { "mediaType": package.config_type, "digest": config_digest, "size": config.len() },
            "layers": layers
        });
        if !package.annotations.is_empty() {
            manifest["annotations"] = json!(package.annotations);
        }
        let manifest = manifest.to_string().into_bytes();
        self.push_raw_manifest(repository, tag, &manifest)
    }
}
