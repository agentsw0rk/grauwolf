/// A kind of object and where the API serves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kind {
    pub group: &'static str,
    pub version: &'static str,
    pub kind: &'static str,
    pub plural: &'static str,
    pub namespaced: bool,
}

impl Kind {
    /// `apiVersion` as in a manifest.
    pub fn api_version(&self) -> String {
        if self.group.is_empty() {
            self.version.to_owned()
        } else {
            format!("{}/{}", self.group, self.version)
        }
    }

    fn base(&self) -> String {
        if self.group.is_empty() {
            format!("/api/{}", self.version)
        } else {
            format!("/apis/{}/{}", self.group, self.version)
        }
    }

    /// The URL path of the collection (`namespace` is ignored for cluster-scoped kinds).
    pub fn collection(&self, namespace: Option<&str>) -> String {
        match (self.namespaced, namespace) {
            (true, Some(namespace)) => {
                format!("{}/namespaces/{namespace}/{}", self.base(), self.plural)
            }
            _ => format!("{}/{}", self.base(), self.plural),
        }
    }

    /// The URL path of one object.
    pub fn object(&self, namespace: Option<&str>, name: &str) -> String {
        format!("{}/{name}", self.collection(namespace))
    }
}

const fn kind(
    group: &'static str,
    version: &'static str,
    kind: &'static str,
    plural: &'static str,
    namespaced: bool,
) -> Kind {
    Kind {
        group,
        version,
        kind,
        plural,
        namespaced,
    }
}

/// The kinds the commands use.
pub mod kinds {
    use super::{Kind, kind};

    pub const NAMESPACE: Kind = kind("", "v1", "Namespace", "namespaces", false);
    pub const SERVICE_ACCOUNT: Kind = kind("", "v1", "ServiceAccount", "serviceaccounts", true);
    pub const CONFIG_MAP: Kind = kind("", "v1", "ConfigMap", "configmaps", true);
    pub const SECRET: Kind = kind("", "v1", "Secret", "secrets", true);
    pub const SERVICE: Kind = kind("", "v1", "Service", "services", true);
    pub const POD: Kind = kind("", "v1", "Pod", "pods", true);
    pub const PERSISTENT_VOLUME_CLAIM: Kind = kind(
        "",
        "v1",
        "PersistentVolumeClaim",
        "persistentvolumeclaims",
        true,
    );
    pub const INGRESS: Kind = kind("networking.k8s.io", "v1", "Ingress", "ingresses", true);
    pub const NETWORK_POLICY: Kind = kind(
        "networking.k8s.io",
        "v1",
        "NetworkPolicy",
        "networkpolicies",
        true,
    );
    pub const NODE: Kind = kind("", "v1", "Node", "nodes", false);
    pub const DEPLOYMENT: Kind = kind("apps", "v1", "Deployment", "deployments", true);
    pub const STATEFUL_SET: Kind = kind("apps", "v1", "StatefulSet", "statefulsets", true);
    pub const DAEMON_SET: Kind = kind("apps", "v1", "DaemonSet", "daemonsets", true);
    pub const JOB: Kind = kind("batch", "v1", "Job", "jobs", true);
    pub const CRON_JOB: Kind = kind("batch", "v1", "CronJob", "cronjobs", true);
    pub const CSI_DRIVER: Kind = kind("storage.k8s.io", "v1", "CSIDriver", "csidrivers", false);
    pub const CRD: Kind = kind(
        "apiextensions.k8s.io",
        "v1",
        "CustomResourceDefinition",
        "customresourcedefinitions",
        false,
    );
    pub const CLUSTER_SPIFFE_ID: Kind = kind(
        "spire.spiffe.io",
        "v1alpha1",
        "ClusterSPIFFEID",
        "clusterspiffeids",
        false,
    );
}
