//! Graph model, node catalog and graph engine (E05) – one engine for builder, interpreter and generated code.

mod catalog;
mod decision_params;
mod engine;
pub mod params;
mod typed;

pub use catalog::{
    CatalogError, Catalogs, ConditionCatalog, ConditionDescriptor, DataType, NodeCatalog,
    NodeKindDescriptor,
};
pub use decision_params::decision_answers;
pub use engine::{
    Checkpoint, CheckpointObserver, ConditionFn, DEFAULT_MAX_STEPS, EngineError, FINISH, Graph,
    GraphBuilder, GraphError, NodeError, NodeFn, START, Scope, TransformFn,
};
pub use typed::{
    NodeData, NodeHandle, TypedGraph, TypedGraphBuilder, erase_condition, erase_node,
    erase_transform,
};
