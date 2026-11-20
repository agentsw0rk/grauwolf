//! Tool packages from OCI registries (ADR-0107): package layout, packing, digest-keyed cache,
//! lockfile and resolution of the `tool_packages` of a spec into verified tool modules. The
//! registry client itself is grauwolf-oci.

mod archive;
mod cache;
pub mod catalog;
mod error;
mod layout;
mod lock;
mod metadata;
mod pack;
mod policy;
mod resolve;
pub mod setup;
#[cfg(feature = "test-registry")]
pub mod testing;
pub mod tree_lock;

pub use archive::source_archive;
pub use cache::PackageCache;
pub use error::PackageFetchError;
pub use layout::{
    ARTIFACT_TYPE, CONFIG_TYPE, MODULE_TYPE, Package, PackageModule, SOURCE_TYPE, TITLE_ANNOTATION,
};
pub use lock::{LOCKFILE_NAME, LockedGit, LockedPackage, LockedSkills, Lockfile};
pub use metadata::{
    ABI_ANNOTATION, AUTHORS_ANNOTATION, BUILT_WITH_ANNOTATION, BuildInfo, CREATED_ANNOTATION,
    DESCRIPTION_ANNOTATION, GRAUWOLF_VERSION, LICENSES_ANNOTATION, REVISION_ANNOTATION,
    RUSTC_ANNOTATION, SOURCE_ANNOTATION, VERSION_ANNOTATION, package_annotations, rfc3339,
};
pub use pack::{ModuleBuilder, PackedPackage, pack_sources};
pub use policy::Policy;
pub use resolve::{
    GitInspection, GitSignatureStatus, PackageResolver, ResolvedSkills, SignatureStatus,
    lock_change,
};

// The registry client moved to grauwolf-oci (ADR-0108); re-exported so that the facade and the
// CLI keep their paths.
pub use grauwolf_git::AllowedSigners;
pub use grauwolf_oci::cosign;
pub use grauwolf_oci::{
    DockerConfig, REGISTRY_AUTH_ENV, RegistryAuth, RegistryClient, default_config_path,
    sha256_digest,
};
