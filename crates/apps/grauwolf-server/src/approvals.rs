//! Approval of tool calls over the SSE stream of a run (ADR-0061).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::Event;
use axum::{Extension, Json};
use grauwolf_agents::events::ToolDecision;
use grauwolf_interpreter::ToolApprover;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::auth::Caller;

/// How long a run waits for a decision before it refuses.
pub const DEFAULT_APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);

/// Pending approvals of all runs.
#[derive(Default)]
pub(crate) struct Approvals {
    pending: Mutex<HashMap<String, Pending>>,
}

/// A pending approval: user of the run and channel for the decision.
struct Pending {
    user: Option<String>,
    decision: oneshot::Sender<ToolDecision>,
}

/// Body of `POST /approvals/{id}`.
#[derive(Deserialize)]
pub(crate) struct Decision {
    approved: bool,
    #[serde(default)]
    reason: Option<String>,
    /// Must match the user of the run (ADR-0060).
    #[serde(default)]
    user_id: Option<String>,
}

/// Random, unguessable ID (128 bits, hexadecimal).
pub(crate) fn new_id() -> String {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).expect("system random source not available");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl Approvals {
    /// Approver for a run of `user`: reports every call as an SSE event `approval` over
    /// `events` and waits at most `timeout` for `POST /approvals/{id}`.
    pub(crate) fn approver(
        self: &Arc<Self>,
        user: Option<String>,
        events: mpsc::UnboundedSender<Event>,
        timeout: Duration,
    ) -> ToolApprover {
        let approvals = Arc::clone(self);
        Arc::new(move |call| {
            let id = new_id();
            let (sender, receiver) = oneshot::channel();
            approvals.pending.lock().unwrap().insert(
                id.clone(),
                Pending {
                    user: user.clone(),
                    decision: sender,
                },
            );
            let request = json!({ "id": id, "tool": call.tool, "args": call.args });
            if let Ok(event) = Event::default().event("approval").json_data(request) {
                let _ = events.send(event);
            }
            let approvals = Arc::clone(&approvals);
            Box::pin(async move {
                let decision = tokio::time::timeout(timeout, receiver).await;
                approvals.pending.lock().unwrap().remove(&id);
                match decision {
                    Ok(Ok(decision)) => decision,
                    _ => ToolDecision::Deny(format!(
                        "No approval given within {} s",
                        timeout.as_secs()
                    )),
                }
            })
        })
    }
}

/// `POST /approvals/{id}`: decides a pending approval; unknown IDs and other
/// users yield `404`. With a user token, the user of the token decides; a different
/// `user_id` in the body yields `403` (ADR-0068).
pub(crate) async fn decide(
    State(approvals): State<Arc<Approvals>>,
    caller: Option<Extension<Caller>>,
    Path(id): Path<String>,
    Json(mut decision): Json<Decision>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    if let Some(Extension(Caller::User(user))) = caller {
        if decision
            .user_id
            .as_ref()
            .is_some_and(|claimed| claimed != &user)
        {
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "user_id does not match the token" })),
            ));
        }
        decision.user_id = Some(user);
    }
    let pending = {
        let mut pending = approvals.pending.lock().unwrap();
        match pending.get(&id) {
            Some(open) if open.user == decision.user_id => pending.remove(&id),
            _ => None,
        }
    };
    let Some(pending) = pending else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("no pending approval '{id}'") })),
        ));
    };
    let _ = pending.decision.send(if decision.approved {
        ToolDecision::Proceed
    } else {
        ToolDecision::Deny(
            decision
                .reason
                .unwrap_or_else(|| "Denied by the user".to_owned()),
        )
    });
    Ok(StatusCode::NO_CONTENT)
}
