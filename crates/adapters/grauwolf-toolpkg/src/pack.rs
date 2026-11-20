//! A packed tool package: the OCI manifest and all blobs, written to and read from an OCI image
//! layout directory (see [`OciImage`]). Packing is deterministic: the same manifest, modules and source give the same digests.

use std::collections::BTreeMap;
use std::ops::Deref;
use std::path::Path;

use grauwolf_oci::{IMAGE_MANIFEST, OciImage};
use grauwolf_tools::PackageManifest;
use serde_json::{Value, json};

use crate::layout::{
    ARTIFACT_TYPE, CONFIG_TYPE, Layout, MODULE_TYPE, SOURCE_TYPE, TITLE_ANNOTATION,
};
use crate::{Package, PackageFetchError, sha256_digest};

/// File name of the source layer in its annotation.
const SOURCE_TITLE: &str = "src.tar.gz";

/// The OCI image of a package: the manifest (image manifest with the artifact type of a tool
/// package) and the blobs it refers to. Derefs to the [`OciImage`], which the registry client
/// pushes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedPackage {
    image: OciImage,
}

impl Deref for PackedPackage {
    type Target = OciImage;

    fn deref(&self) -> &OciImage {
        &self.image
    }
}

impl PackedPackage {
    /// Packs `manifest` with its `modules` (file name → WASM) and the optional source layer
    /// (a `.tar.gz`, see [`crate::source_archive`]).
    pub fn new(
        manifest: &PackageManifest,
        modules: &BTreeMap<String, Vec<u8>>,
        source: Option<Vec<u8>>,
    ) -> Result<Self, PackageFetchError> {
        let mut blobs = BTreeMap::new();
        let mut add = |bytes: Vec<u8>| {
            let digest = sha256_digest(&bytes);
            let size = bytes.len();
            blobs.insert(digest.clone(), bytes);
            (digest, size)
        };
        let (config_digest, config_size) = add(manifest.to_json().into_bytes());
        let mut layers = Vec::new();
        let mut used: Vec<&str> = manifest.tools.iter().map(|t| t.module.as_str()).collect();
        used.sort_unstable();
        used.dedup();
        for name in used {
            let bytes = modules.get(name).ok_or_else(|| {
                PackageFetchError::InvalidPackage(format!(
                    "the module '{name}' is missing for packing"
                ))
            })?;
            let (digest, size) = add(bytes.clone());
            layers.push(layer(MODULE_TYPE, &digest, size, name));
        }
        if let Some(source) = source {
            let (digest, size) = add(source);
            layers.push(layer(SOURCE_TYPE, &digest, size, SOURCE_TITLE));
        }
        let manifest_json = json!({
            "schemaVersion": 2,
            "mediaType": IMAGE_MANIFEST,
            "artifactType": ARTIFACT_TYPE,
            "config": { "mediaType": CONFIG_TYPE, "digest": config_digest, "size": config_size },
            "layers": layers,
        })
        .to_string()
        .into_bytes();
        Ok(Self {
            image: OciImage {
                digest: sha256_digest(&manifest_json),
                manifest: manifest_json,
                blobs,
            },
        })
    }

    /// The same package with `annotations` on its OCI manifest (ADR-0124, see
    /// [`crate::package_annotations`]); replaces annotations set before. The digest changes.
    pub fn with_annotations(mut self, annotations: BTreeMap<String, String>) -> Self {
        let mut manifest: Value =
            serde_json::from_slice(&self.image.manifest).expect("a packed manifest is JSON");
        if annotations.is_empty() {
            manifest
                .as_object_mut()
                .map(|object| object.remove("annotations"));
        } else {
            manifest["annotations"] = json!(annotations);
        }
        let bytes = manifest.to_string().into_bytes();
        self.image.digest = sha256_digest(&bytes);
        self.image.manifest = bytes;
        self
    }

    /// The verified package this packed package describes (what a pull would give).
    pub fn package(&self) -> Result<Package, PackageFetchError> {
        Layout::parse(&self.manifest)?.assemble(&self.digest, |digest| {
            self.blobs.get(digest).cloned().ok_or_else(|| {
                PackageFetchError::InvalidPackage(format!("blob {digest} is missing"))
            })
        })
    }

    /// Writes the OCI image layout into `dir` (created if needed; existing blobs are replaced).
    pub fn write_layout(&self, dir: &Path) -> Result<(), PackageFetchError> {
        Ok(self.image.write_layout(dir, ARTIFACT_TYPE)?)
    }

    /// Reads an OCI image layout written by [`Self::write_layout`], verifies every blob against
    /// its digest and checks that it describes a tool package.
    pub fn read_layout(dir: &Path) -> Result<Self, PackageFetchError> {
        let image = OciImage::read_layout(dir)?;
        Layout::parse(&image.manifest)?;
        Ok(Self { image })
    }
}

fn layer(media_type: &str, digest: &str, size: usize, title: &str) -> Value {
    json!({
        "mediaType": media_type, "digest": digest, "size": size,
        "annotations": { TITLE_ANNOTATION: title },
    })
}

/// Builds the WASM module of a tool from its definition and code (the isolated tool build,
/// ADR-0030). A trait because adapters do not depend on each other: the facade implements it
/// with `grauwolf-toolbuild`.
pub trait ModuleBuilder: Send + Sync {
    /// Makes the build dependencies available (once, before the first build).
    fn prepare(&self) -> Result<(), String>;
    /// Builds `tool` (definition and code body) into a module.
    fn build(&self, tool: &grauwolf_spec::ToolSpec) -> Result<Vec<u8>, String>;
}

/// Packs a package folder given by its files: `manifest_bytes` is the `grauwolf-tools.json`
/// (source form, see `PackageManifest::from_source_json`), `read("src/<tool>.rs")` returns the
/// code body of a tool, `label(relative)` names a file in error messages. Every module is
/// built from the sources with `builder`; the same sources give the same package (apart from
/// what the toolchain produces).
pub fn pack_sources(
    manifest_bytes: Vec<u8>,
    read: &mut dyn FnMut(&str) -> Result<Vec<u8>, String>,
    label: &dyn Fn(&str) -> String,
    builder: &dyn ModuleBuilder,
) -> Result<PackedPackage, String> {
    use grauwolf_tools::MANIFEST_FILE;
    let manifest = PackageManifest::from_source_json(&String::from_utf8_lossy(&manifest_bytes))
        .map_err(|error| format!("{}: {error}", label(MANIFEST_FILE)))?;
    builder
        .prepare()
        .map_err(|error| format!("tool dependencies not prepared: {error}"))?;
    let mut modules = BTreeMap::new();
    let mut source = BTreeMap::from([(MANIFEST_FILE.to_owned(), manifest_bytes)]);
    for tool in &manifest.tools {
        let relative = format!("src/{}.rs", tool.name);
        let code_bytes = read(&relative)?;
        let code = String::from_utf8(code_bytes.clone())
            .map_err(|_| format!("{}: not UTF-8 text", label(&relative)))?;
        let module = builder
            .build(&grauwolf_spec::ToolSpec {
                name: tool.name.clone(),
                description: tool.description.clone(),
                args: tool.args.clone(),
                code,
            })
            .map_err(|error| format!("{}: {error}", label(&relative)))?;
        if modules.insert(tool.module.clone(), module).is_some() {
            return Err(format!(
                "{}: the module '{}' is used by more than one tool",
                label(MANIFEST_FILE),
                tool.module
            ));
        }
        source.insert(relative, code_bytes);
    }
    let archive = crate::source_archive(&source).map_err(|error| error.to_string())?;
    PackedPackage::new(&manifest, &modules, Some(archive)).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> PackageManifest {
        PackageManifest::from_source_json(
            r#"{"name":"p","version":"1.0.0","tools":[
                {"name":"b","description":"B","args":{"type":"object"}},
                {"name":"a","description":"A","args":{"type":"object"}}]}"#,
        )
        .unwrap()
    }

    fn modules() -> BTreeMap<String, Vec<u8>> {
        BTreeMap::from([
            ("a.wasm".to_owned(), b"module a".to_vec()),
            ("b.wasm".to_owned(), b"module b".to_vec()),
        ])
    }

    fn temp(name: &str) -> grauwolf_test::TestDir {
        grauwolf_test::TestDir::new(&format!("pack-{name}"))
    }

    #[test]
    fn packing_twice_gives_the_same_digest() {
        let first = PackedPackage::new(&manifest(), &modules(), Some(b"src".to_vec())).unwrap();
        let second = PackedPackage::new(&manifest(), &modules(), Some(b"src".to_vec())).unwrap();
        assert_eq!(first.digest, second.digest);
        assert_eq!(first.digest, sha256_digest(&first.manifest));
    }

    #[test]
    fn a_different_module_changes_the_digest() {
        let mut other = modules();
        other.insert("a.wasm".into(), b"changed".to_vec());
        let first = PackedPackage::new(&manifest(), &modules(), None).unwrap();
        let second = PackedPackage::new(&manifest(), &other, None).unwrap();
        assert_ne!(first.digest, second.digest);
    }

    #[test]
    fn a_missing_module_is_an_error() {
        let mut modules = modules();
        modules.remove("b.wasm");
        let error = PackedPackage::new(&manifest(), &modules, None).unwrap_err();
        assert!(error.to_string().contains("b.wasm"), "{error}");
    }

    #[test]
    fn the_packed_package_assembles_like_a_pulled_one() {
        let packed = PackedPackage::new(&manifest(), &modules(), Some(b"src".to_vec())).unwrap();
        let package = packed.package().unwrap();
        assert_eq!(package.digest, packed.digest);
        assert_eq!(package.manifest, manifest());
        assert_eq!(package.modules["a.wasm"].bytes, b"module a");
        assert_eq!(package.source.as_deref(), Some(&b"src"[..]));
    }

    #[test]
    fn the_layout_round_trips() {
        let packed = PackedPackage::new(&manifest(), &modules(), Some(b"src".to_vec())).unwrap();
        let dir = temp("round-trip");
        packed.write_layout(&dir).unwrap();
        assert!(dir.join("oci-layout").is_file());
        assert!(dir.join("index.json").is_file());
        assert_eq!(PackedPackage::read_layout(&dir).unwrap(), packed);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_damaged_blob_is_detected_when_reading() {
        let packed = PackedPackage::new(&manifest(), &modules(), None).unwrap();
        let dir = temp("damaged");
        packed.write_layout(&dir).unwrap();
        let module = sha256_digest(b"module a");
        let hex = module.strip_prefix("sha256:").unwrap();
        std::fs::write(dir.join("blobs").join("sha256").join(hex), b"tampered").unwrap();
        let error = PackedPackage::read_layout(&dir).unwrap_err();
        assert!(
            matches!(error, PackageFetchError::DigestMismatch { .. }),
            "{error}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_directory_that_is_no_layout_is_an_error() {
        let dir = temp("empty");
        std::fs::create_dir_all(&dir).unwrap();
        let error = PackedPackage::read_layout(&dir).unwrap_err();
        assert!(error.to_string().contains("oci-layout"), "{error}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
