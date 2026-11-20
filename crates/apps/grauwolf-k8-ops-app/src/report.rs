use grauwolf_k8s::Outcome as K8s;
use grauwolf_openbao::Outcome as Bao;

pub fn k8s(outcome: K8s) -> &'static str {
    match outcome {
        K8s::Created => "created",
        K8s::Updated => "updated",
        K8s::Unchanged => "unchanged",
    }
}

pub fn bao(outcome: Bao) -> &'static str {
    match outcome {
        Bao::Created => "created",
        Bao::Updated => "updated",
        Bao::Unchanged => "unchanged",
    }
}
