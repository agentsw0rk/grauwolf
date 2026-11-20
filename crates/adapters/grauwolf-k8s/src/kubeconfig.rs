//! A kubeconfig given as text, e.g. pasted into grauwolf studio (ADR-0129 B1): which contexts it
//! offers, without connecting.

/// The contexts of a kubeconfig, each with the server of its cluster, and its current context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KubeconfigContexts {
    pub contexts: Vec<(String, Option<String>)>,
    pub current: Option<String>,
}

/// Reads the contexts of the kubeconfig `text`; an error when it is no kubeconfig.
pub fn kubeconfig_contexts(text: &str) -> Result<KubeconfigContexts, String> {
    let file = kube::config::Kubeconfig::from_yaml(text)
        .map_err(|error| format!("not a kubeconfig: {error}"))?;
    let server_of = |cluster: &str| {
        file.clusters
            .iter()
            .find(|named| named.name == cluster)
            .and_then(|named| named.cluster.as_ref())
            .and_then(|cluster| cluster.server.clone())
    };
    let contexts = file
        .contexts
        .iter()
        .map(|named| {
            let server = named
                .context
                .as_ref()
                .and_then(|context| server_of(&context.cluster));
            (named.name.clone(), server)
        })
        .collect();
    Ok(KubeconfigContexts {
        contexts,
        current: file.current_context.clone(),
    })
}
