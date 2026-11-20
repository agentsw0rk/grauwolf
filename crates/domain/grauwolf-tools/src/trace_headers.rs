//! HTTP headers that continue the trace of the calling run (ADR-0123).
//!
//! A tool that talks HTTP to another agent (A2A, MCP) cannot be handed the trace context by its
//! arguments. The run wraps the call in [`with_trace_headers`]; the client reads
//! [`current_trace_headers`] when it builds a request. The scope is bound to the future, not to
//! a thread or a runtime: it holds across `.await` points inside the wrapped future and ends
//! with it, and a task that the call spawns does not see it. Domain crates only carry
//! `(name, value)` pairs, they know nothing of OpenTelemetry.

use std::cell::RefCell;
use std::future::Future;

thread_local! {
    static CURRENT: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// The headers of the enclosing [`with_trace_headers`] scope (`traceparent`, `tracestate`);
/// empty outside of one or when the run is not traced.
pub fn current_trace_headers() -> Vec<(String, String)> {
    CURRENT.with(|current| current.borrow().clone())
}

/// Runs `future` with `headers` as [`current_trace_headers`]. Without headers, the scope of an
/// enclosing call stays as it is (a sub-agent without tracing continues the trace of its caller).
pub async fn with_trace_headers<F: Future>(headers: Vec<(String, String)>, future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    std::future::poll_fn(|cx| {
        if headers.is_empty() {
            return future.as_mut().poll(cx);
        }
        let outer = CURRENT.with(|current| current.replace(headers.clone()));
        // Restores the outer scope even when the poll panics.
        struct Restore(Vec<(String, String)>);
        impl Drop for Restore {
            fn drop(&mut self) {
                CURRENT.with(|current| *current.borrow_mut() = std::mem::take(&mut self.0));
            }
        }
        let _restore = Restore(outer);
        future.as_mut().poll(cx)
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::task::Poll;

    use futures_executor::block_on;

    use super::*;

    fn header(value: &str) -> Vec<(String, String)> {
        vec![("traceparent".to_owned(), value.to_owned())]
    }

    #[test]
    fn there_are_no_headers_outside_of_a_scope() {
        assert!(current_trace_headers().is_empty());
    }

    #[test]
    fn the_headers_hold_across_awaits_inside_the_scope_and_end_with_it() {
        let seen = block_on(with_trace_headers(header("a"), async {
            futures_util::future::ready(()).await;
            std::future::poll_fn(|cx| {
                cx.waker().wake_by_ref();
                Poll::Ready(())
            })
            .await;
            current_trace_headers()
        }));
        assert_eq!(seen, header("a"));
        assert!(current_trace_headers().is_empty());
    }

    #[test]
    fn an_inner_scope_wins_and_an_empty_one_keeps_the_outer() {
        let seen = block_on(with_trace_headers(header("outer"), async {
            let empty = with_trace_headers(Vec::new(), async { current_trace_headers() }).await;
            let inner =
                with_trace_headers(header("inner"), async { current_trace_headers() }).await;
            (empty, inner, current_trace_headers())
        }));
        assert_eq!(seen, (header("outer"), header("inner"), header("outer")));
    }
}
