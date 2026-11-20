//! Git tools for one repository (ADR-0104, ADR-0108): libgit2 with an own smart-HTTP transport.

mod git;

pub use git::{GIT_TOOLS, GitConfig, GitTools, PushPolicy};
