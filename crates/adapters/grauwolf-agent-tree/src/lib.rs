//! The prepared agent tree (ADR-0115) as a library below the apps: a spec and the specs of its
//! sub-agents, read and checked once with one set of rules, then resolved (git skills, tool
//! packages, code tools) and, for an image, copied. The host and `grauwolf-tools` both use it, so
//! `spec validate`, `run`, `generate` and `tools lock` agree on what a tree is (ADR-0118).
//!
//! Checks that need more than this crate knows are options of the read phase ([`ReadOptions`]):
//! the model providers the program supports, and whether local skill directories are checked.

mod build;
mod harness;
mod skills;
mod spec_file;
mod tree;

pub use build::build_tools;
pub use grauwolf_generator::CompanionKind;
pub use harness::harness_registry;
pub use skills::{check_local, replace_with_dir};
pub use spec_file::{
    FileReference, ReferenceKind, file_references, read_spec, resolve_code_file,
    resolve_file_references, warning_lines,
};
pub use tree::{
    APP_TOOL_CACHE, AgentNode, AgentTree, Child, ReadErrors, ReadOptions, Resolution,
    ResolveOptions, ResolvedSkill,
};
