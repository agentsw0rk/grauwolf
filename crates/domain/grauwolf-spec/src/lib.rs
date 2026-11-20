//! AgentSpec (E13): loading with strategy expansion and semantic validation of the DSL.
//!
//! The data model (types, parsing without expansion, JSON schema, read-only helpers) lives in
//! `grauwolf-spec-model` and is re-exported here, so every path `grauwolf_spec::…` stays valid
//! (ADR-0108 stage 6). What cannot be an inherent method of a type from another crate –
//! `from_json`, `validate`, `validate_with`, `warnings`, `warnings_with` – is provided by the
//! extension trait [`AgentSpecExt`]; bring it into scope with
//! `use grauwolf_spec::AgentSpecExt;`.

mod validate;

use grauwolf_graph::Catalogs;
pub use grauwolf_spec_model::*;
pub use validate::{SpecWarning, ValidationError, ValidationErrorKind};

/// Loading and checking of an [`AgentSpec`]: the methods that need strategy expansion or the
/// validation rules and therefore cannot live in `grauwolf-spec-model`.
pub trait AgentSpecExt: Sized {
    /// Loads a spec from a JSON string: parses it (see [`AgentSpec::parse`]) and expands the
    /// `strategy` into the graph.
    fn from_json(json: &str) -> Result<Self, SpecError>;

    /// Checks the spec against the built-in catalogs and returns all errors found.
    fn validate(&self) -> Result<(), Vec<ValidationError>>;

    /// Checks the spec against `catalogs` and returns all errors found.
    fn validate_with(&self, catalogs: &Catalogs) -> Result<(), Vec<ValidationError>>;

    /// Errors for `agent` nodes and `for_each` agent bodies whose declared types differ from the
    /// signature (input type, output type) of the sub-agent's own spec; `signatures` maps the
    /// names in `agent_tools` to them. Agents without a signature are not checked (ADR-0101).
    fn check_sub_agent_types(
        &self,
        signatures: &std::collections::BTreeMap<String, (String, String)>,
    ) -> Vec<ValidationError>;

    /// The pieces of code of the graphs (ADR-0102): `code` nodes, `on_code` conditions and edge
    /// transformations, with stable names and the JSON pointer of their code. Meant for
    /// validated specs; pieces with invalid parameters or types are left out.
    fn code_pieces(&self) -> Vec<CodePiece>;

    /// Warnings of the spec against the built-in catalogs; they never fail the validation.
    fn warnings(&self) -> Vec<SpecWarning>;

    /// Warnings of the spec against `catalogs`, in graph order.
    fn warnings_with(&self, catalogs: &Catalogs) -> Vec<SpecWarning>;
}

impl AgentSpecExt for AgentSpec {
    fn from_json(json: &str) -> Result<Self, SpecError> {
        let mut spec = AgentSpec::parse(json)?;
        if let Some(strategy) = &spec.strategy {
            if !spec.graph.nodes.is_empty() || !spec.graph.edges.is_empty() {
                return Err(SpecError::Strategy(
                    "`strategy` and `graph` are mutually exclusive".into(),
                ));
            }
            let expansion = grauwolf_strategies::expand(strategy).map_err(SpecError::Strategy)?;
            for (name, schema) in expansion.types {
                if spec.types.contains_key(&name) {
                    return Err(SpecError::Strategy(format!(
                        "type '{name}' is reserved for the strategy '{}'",
                        strategy.kind
                    )));
                }
                spec.types.insert(name, schema);
            }
            spec.graph = expansion.graph;
        }
        Ok(spec)
    }

    fn validate(&self) -> Result<(), Vec<ValidationError>> {
        validate::validate(self)
    }

    fn validate_with(&self, catalogs: &Catalogs) -> Result<(), Vec<ValidationError>> {
        validate::validate_with(self, catalogs)
    }

    fn check_sub_agent_types(
        &self,
        signatures: &std::collections::BTreeMap<String, (String, String)>,
    ) -> Vec<ValidationError> {
        validate::check_sub_agent_types(self, signatures)
    }

    fn code_pieces(&self) -> Vec<CodePiece> {
        validate::code_pieces(self)
    }

    fn warnings(&self) -> Vec<SpecWarning> {
        validate::warnings(self)
    }

    fn warnings_with(&self, catalogs: &Catalogs) -> Vec<SpecWarning> {
        validate::warnings_with(self, catalogs)
    }
}
