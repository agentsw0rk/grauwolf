//! The delegation of a run (ADR-0111): the Txn-Token that internal outgoing calls carry.
//!
//! A run that acts on behalf of a person holds a [`Delegation`]. The service wraps the whole run
//! in [`with_delegation`]; the HTTP clients of internal calls (A2A, MCP over HTTP, `fetch` to hosts
//! with a workload identity) read [`current_delegation`] when they build a request and add the
//! `Txn-Token` header. As with the trace headers, the scope belongs to the future: it holds across
//! `.await` points inside it, a sub-agent in the same process sees it, a spawned task does not.
//! The token never travels through tool arguments, so the model never sees it.

use std::cell::RefCell;
use std::future::Future;
use std::sync::Arc;

use futures_util::future::BoxFuture;

/// The name of the header that carries the Txn-Token (ADR-0111).
pub const TXN_TOKEN_HEADER: &str = "Txn-Token";

/// The Txn-Token of a run. The holder renews it before it expires; callers ask right before
/// every outgoing request.
pub trait Delegation: Send + Sync {
    /// The current Txn-Token (renewed if it is about to expire). A credential: never log it,
    /// never put it into an error.
    fn txn_token(&self) -> BoxFuture<'_, Result<String, String>>;
}

thread_local! {
    static CURRENT: RefCell<Option<Arc<dyn Delegation>>> = const { RefCell::new(None) };
}

/// The delegation of the enclosing [`with_delegation`] scope; `None` outside of one.
pub fn current_delegation() -> Option<Arc<dyn Delegation>> {
    CURRENT.with(|current| current.borrow().clone())
}

/// The `Txn-Token` header for an internal outgoing request of the current run, if it acts on
/// behalf of a person. The error says why the token could not be renewed (no token in it).
pub async fn delegation_header() -> Result<Option<(&'static str, String)>, String> {
    match current_delegation() {
        Some(delegation) => Ok(Some((TXN_TOKEN_HEADER, delegation.txn_token().await?))),
        None => Ok(None),
    }
}

/// Runs `future` with `delegation` as [`current_delegation`]. `None` keeps the scope of an
/// enclosing call.
pub async fn with_delegation<F: Future>(
    delegation: Option<Arc<dyn Delegation>>,
    future: F,
) -> F::Output {
    let mut future = std::pin::pin!(future);
    std::future::poll_fn(|cx| {
        let Some(delegation) = &delegation else {
            return future.as_mut().poll(cx);
        };
        let outer = CURRENT.with(|current| current.replace(Some(Arc::clone(delegation))));
        // Restores the outer scope even when the poll panics.
        struct Restore(Option<Arc<dyn Delegation>>);
        impl Drop for Restore {
            fn drop(&mut self) {
                CURRENT.with(|current| *current.borrow_mut() = self.0.take());
            }
        }
        let _restore = Restore(outer);
        future.as_mut().poll(cx)
    })
    .await
}

#[cfg(test)]
mod tests {
    use futures_executor::block_on;

    use super::*;

    struct Fixed(&'static str);

    impl Delegation for Fixed {
        fn txn_token(&self) -> BoxFuture<'_, Result<String, String>> {
            Box::pin(async move { Ok(self.0.to_owned()) })
        }
    }

    fn fixed(token: &'static str) -> Option<Arc<dyn Delegation>> {
        Some(Arc::new(Fixed(token)))
    }

    #[test]
    fn there_is_no_header_outside_of_a_scope() {
        assert_eq!(block_on(delegation_header()), Ok(None));
    }

    #[test]
    fn the_header_holds_inside_the_scope_and_an_empty_scope_keeps_the_outer() {
        let seen = block_on(with_delegation(fixed("outer"), async {
            let kept = with_delegation(None, delegation_header()).await;
            let inner = with_delegation(fixed("inner"), delegation_header()).await;
            (kept, inner, delegation_header().await)
        }));
        let header = |value: &str| Ok(Some((TXN_TOKEN_HEADER, value.to_owned())));
        assert_eq!(seen, (header("outer"), header("inner"), header("outer")));
        assert_eq!(block_on(delegation_header()), Ok(None));
    }
}
