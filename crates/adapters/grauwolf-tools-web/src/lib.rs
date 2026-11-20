//! The `fetch` tool: HTTP requests to allow-listed hosts (ADR-0098, ADR-0108).

mod fetch;
mod html;
mod page;

pub use fetch::{FETCH_TOOLS, FetchConfig, Resolver, SystemResolver, fetch_tool, fetch_tool_with};
pub use html::html_to_text;
pub use page::{MAX_PAGE_BYTES, Page, Validators, fetch_page};
