//! Kubernetes access for `grauwolf-k8-ops` (ADR-0120): objects as JSON values over kube-rs.

mod cluster;
mod image_build;
mod kind;
mod kubeconfig;
#[cfg(feature = "testing")]
pub mod testing;

pub use cluster::{APPLIED_HASH, Cluster, FIELD_MANAGER, K8sError, Outcome, hash_of};
pub use image_build::{
    BUILD_NAMESPACE, Build, BuildFinished, BuildRequest, KANIKO_IMAGE, kaniko_job,
    kaniko_repository, registry_secret, run_build, run_build_with,
};
pub use kind::{Kind, kinds};
pub use kubeconfig::{KubeconfigContexts, kubeconfig_contexts};
