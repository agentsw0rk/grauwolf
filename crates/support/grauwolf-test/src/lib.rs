//! Test helpers (E12): mock executor, mock tools, event recording.

pub mod conformance;
mod mock_llm;
mod temp_dir;

pub use mock_llm::MockLlmClient;
pub use temp_dir::TestDir;
