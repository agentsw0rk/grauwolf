//! Agent core, graph strategies and feature pipeline (E04–E06).

mod accounting;
pub mod approval;
pub mod checkpoints;
pub mod code;
mod compression;
pub mod conditions;
mod context;
mod conversation;
pub mod decision;
mod event_bus;
pub mod events;
mod facts;
pub mod flows;
pub mod for_each;
pub mod implementations;
pub mod knowledge;
mod llm_gateway;
pub mod memory;
mod model_selection;
pub mod nodes;
pub mod observer;
pub mod parallel;
pub mod run_handles;
pub mod run_lifecycle;
pub mod scopes;
mod storage;
pub mod structured;
mod tool_access;
mod tool_execution;

pub use accounting::Budget;
pub use compression::HistoryCompression;
pub use context::{AgentContext, FrameSink};
pub use event_bus::MODERATION_PROMPT_ID;
pub use structured::Structured;
