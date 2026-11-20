//! Tool registry, tool subsets of the entered subgraphs and permission to build tools
//! (ADR-0034, ADR-0075).

use grauwolf_prompt::ToolDescriptor;
use grauwolf_tools::{NewTool, ToolRegistry};

/// Entry in the stack of tool scopes: a tool subset or a build permission (ADR-0075).
#[derive(Debug, Clone)]
struct ToolScope {
    names: Option<Vec<String>>,
    building: Option<bool>,
}

/// The tools of a run and which of them are offered and allowed in the active section.
#[derive(Clone, Default)]
pub(crate) struct ToolAccess {
    tools: ToolRegistry,
    /// Tool subsets and build permissions of the entered subgraphs (ADR-0034, ADR-0075).
    scopes: Vec<ToolScope>,
    /// Name of the tool for building, and whether building is allowed outside subgraphs (ADR-0075).
    self_tools: Option<(String, bool)>,
}

impl ToolAccess {
    /// Access with an empty registry and no scopes.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Makes `tools` available.
    pub(crate) fn set_registry(&mut self, tools: ToolRegistry) {
        self.tools = tools;
    }

    /// The registry with all tools, regardless of the scopes.
    pub(crate) fn registry(&self) -> &ToolRegistry {
        &self.tools
    }

    /// Definitions of the self-made tools, for checkpoints (ADR-0077).
    pub(crate) fn self_made(&self) -> Vec<NewTool> {
        self.tools.self_made()
    }

    /// Tools offered in the active section.
    pub(crate) fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools
            .descriptors()
            .into_iter()
            .filter(|tool| self.is_allowed(&tool.name))
            .collect()
    }

    /// Restricts the tools to `tools` until the matching [`ToolAccess::leave_scope`].
    pub(crate) fn enter_tool_scope(&mut self, tools: Vec<String>) {
        self.scopes.push(ToolScope {
            names: Some(tools),
            building: None,
        });
    }

    /// Allows or forbids building and using self-built tools until the matching
    /// [`ToolAccess::leave_scope`], also against an outer tool subset (ADR-0075).
    pub(crate) fn enter_building_scope(&mut self, allowed: bool) {
        self.scopes.push(ToolScope {
            names: None,
            building: Some(allowed),
        });
    }

    /// Lifts the most recently set tool subset or build permission.
    pub(crate) fn leave_scope(&mut self) {
        self.scopes.pop();
    }

    /// `tool` builds new tools; outside subgraphs only when `enabled` (ADR-0075).
    pub(crate) fn set_self_tools(&mut self, tool: String, enabled: bool) {
        self.self_tools = Some((tool, enabled));
    }

    /// May the tool `name` be offered and called in the active section?
    pub(crate) fn is_allowed(&self, name: &str) -> bool {
        let list = self.scopes.iter().rposition(|scope| scope.names.is_some());
        if let Some((tool, enabled)) = &self.self_tools
            && (name == tool || self.tools.is_on_shelf(name))
        {
            // Innermost build permission; a tool list further inside hides it.
            let grant = self
                .scopes
                .iter()
                .rposition(|scope| scope.building.is_some());
            return match grant {
                Some(grant) if list.is_none_or(|list| grant > list) => {
                    self.scopes[grant].building == Some(true)
                }
                Some(_) => false,
                None => list.is_none() && *enabled,
            };
        }
        list.is_none_or(|list| {
            self.scopes[list]
                .names
                .iter()
                .flatten()
                .any(|tool| tool == name)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures_util::future::BoxFuture;
    use grauwolf_tools::{DynTool, ToolError, ToolShelf};
    use serde_json::{Value, json};

    use super::*;

    struct Named(&'static str);

    impl DynTool for Named {
        fn name(&self) -> &str {
            self.0
        }
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: self.0.to_owned(),
                description: String::new(),
                parameters: json!({ "type": "object" }),
            }
        }
        fn call_json(&self, _args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
            Box::pin(async { Ok(String::new()) })
        }
    }

    fn access(names: &[&'static str]) -> ToolAccess {
        let mut builder = ToolRegistry::builder();
        for name in names {
            builder = builder.tool(Named(name));
        }
        let mut access = ToolAccess::new();
        access.set_registry(builder.build());
        access
    }

    fn offered(access: &ToolAccess) -> Vec<String> {
        access
            .descriptors()
            .into_iter()
            .map(|tool| tool.name)
            .collect()
    }

    /// Access with tools `a`, `b`, the builder tool `build` and a self-made tool `made` on the shelf.
    fn with_shelf(enabled: bool) -> ToolAccess {
        let shelf = ToolShelf::default();
        shelf.put(Arc::new(Named("made"))).unwrap();
        let mut access = ToolAccess::new();
        access.set_registry(
            ToolRegistry::builder()
                .tool(Named("a"))
                .tool(Named("b"))
                .tool(Named("build"))
                .build()
                .with_shelf(shelf),
        );
        access.set_self_tools("build".into(), enabled);
        access
    }

    #[test]
    fn without_scopes_every_tool_is_allowed() {
        let access = access(&["a", "b"]);
        assert_eq!(offered(&access), ["a", "b"]);
        assert!(access.is_allowed("a"));
    }

    #[test]
    fn a_tool_scope_restricts_to_its_names_until_left() {
        let mut access = access(&["a", "b", "c"]);
        access.enter_tool_scope(vec!["b".into()]);
        assert_eq!(offered(&access), ["b"]);
        assert!(!access.is_allowed("a"));
        access.leave_scope();
        assert_eq!(offered(&access), ["a", "b", "c"]);
    }

    #[test]
    fn the_innermost_tool_subset_wins() {
        let mut access = access(&["a", "b", "c"]);
        access.enter_tool_scope(vec!["a".into(), "b".into()]);
        access.enter_tool_scope(vec!["c".into()]);
        assert_eq!(offered(&access), ["c"]);
        access.leave_scope();
        assert_eq!(offered(&access), ["a", "b"]);
    }

    #[test]
    fn leaving_without_a_scope_does_nothing() {
        let mut access = access(&["a"]);
        access.leave_scope();
        assert_eq!(offered(&access), ["a"]);
    }

    #[test]
    fn building_is_off_outside_subgraphs_when_not_enabled() {
        let access = with_shelf(false);
        assert_eq!(offered(&access), ["a", "b"]);
        assert!(!access.is_allowed("build"));
        assert!(!access.is_allowed("made"));
    }

    #[test]
    fn building_is_on_outside_subgraphs_when_enabled() {
        let access = with_shelf(true);
        assert!(access.is_allowed("build"));
        assert!(access.is_allowed("made"));
    }

    #[test]
    fn a_building_scope_overrides_the_default_in_both_directions() {
        let mut off = with_shelf(true);
        off.enter_building_scope(false);
        assert!(!off.is_allowed("build") && !off.is_allowed("made"));
        assert!(off.is_allowed("a"));
        off.leave_scope();
        assert!(off.is_allowed("build"));

        let mut on = with_shelf(false);
        on.enter_building_scope(true);
        assert!(on.is_allowed("build") && on.is_allowed("made"));
    }

    #[test]
    fn a_tool_subset_inside_a_building_scope_hides_the_builder() {
        let mut access = with_shelf(true);
        access.enter_building_scope(true);
        access.enter_tool_scope(vec!["a".into()]);
        assert!(!access.is_allowed("build"));
        assert!(!access.is_allowed("made"));
        assert!(access.is_allowed("a"));
    }

    #[test]
    fn a_building_scope_inside_a_tool_subset_grants_the_builder() {
        let mut access = with_shelf(false);
        access.enter_tool_scope(vec!["a".into()]);
        access.enter_building_scope(true);
        assert!(access.is_allowed("build"));
        assert!(access.is_allowed("made"));
        assert!(access.is_allowed("a"));
        assert!(!access.is_allowed("b"));
    }

    #[test]
    fn a_tool_subset_without_a_building_scope_hides_the_builder_even_if_enabled() {
        let mut access = with_shelf(true);
        access.enter_tool_scope(vec!["a".into(), "build".into()]);
        assert!(!access.is_allowed("build"));
    }

    #[test]
    fn clone_has_its_own_scope_stack() {
        let mut access = access(&["a", "b"]);
        let fork = access.clone();
        access.enter_tool_scope(vec!["a".into()]);
        assert_eq!(offered(&fork), ["a", "b"]);
        assert_eq!(offered(&access), ["a"]);
    }

    #[test]
    fn self_made_is_empty_without_built_tools() {
        assert!(with_shelf(true).self_made().is_empty());
    }
}
