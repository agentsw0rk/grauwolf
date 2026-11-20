//! The tool namespace of a spec (ADR-0025, ADR-0117): every tool with its name, its origin and the
//! JSON pointer of the place that declares it. Everything that asks "is this name taken" or "which
//! tools does the spec bring" reads this list.

use std::fmt;

use crate::{AgentSpec, BUILTIN_SKILL_TOOLS, escape_pointer_token};

/// Where a tool comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolOrigin {
    /// A tool of `tools` (WASM).
    Own,
    /// A tool of an MCP server (`mcp_servers.*.tools`).
    Mcp,
    /// A resource tool of an MCP server (`<server>_list_resources`, `<server>_read_resource`).
    McpResource,
    /// The search tool of `knowledge`.
    Knowledge,
    /// A memory tool of `long_term_memory`.
    Memory,
    /// A tool of `builtin_tools`.
    Builtin,
    /// A skill tool (`load_skill`, `read_skill_file`); offered when the spec has `skills`.
    Skill,
    /// A sub-agent (`agent_tools`).
    SubAgent,
    /// A tool of a tool package whose name is known without fetching the package, after renaming.
    Package,
    /// The tool for building tools (`self_tools.tool`).
    SelfBuild,
    /// A code piece of a graph (ADR-0102); it is built and shipped like a tool.
    CodePiece,
}

impl ToolOrigin {
    /// `true` for the origins that come from outside the spec's own tools: what
    /// [`AgentSpec::external_tools`] lists.
    pub fn is_external(self) -> bool {
        matches!(
            self,
            Self::Mcp
                | Self::McpResource
                | Self::Knowledge
                | Self::Memory
                | Self::Builtin
                | Self::Skill
                | Self::SubAgent
        )
    }
}

impl fmt::Display for ToolOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Own => "own tool",
            Self::Mcp => "MCP tool",
            Self::McpResource => "MCP resource tool",
            Self::Knowledge => "knowledge search tool",
            Self::Memory => "memory tool",
            Self::Builtin => "built-in tool",
            Self::Skill => "skill tool",
            Self::SubAgent => "sub-agent",
            Self::Package => "package tool",
            Self::SelfBuild => "self-build tool",
            Self::CodePiece => "code piece",
        })
    }
}

/// One name of the tool namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolEntry {
    /// The name under which the model calls the tool.
    pub name: String,
    /// Where the tool comes from.
    pub origin: ToolOrigin,
    /// JSON pointer of the place that declares the tool.
    pub pointer: String,
}

impl ToolEntry {
    fn new(name: impl Into<String>, origin: ToolOrigin, pointer: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            origin,
            pointer: pointer.into(),
        }
    }
}

/// A name that an earlier entry already has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolCollision<'a> {
    /// The later declaration: where the collision is reported.
    pub later: &'a ToolEntry,
    /// The first earlier declaration of the same name.
    pub earlier: &'a ToolEntry,
}

/// All tools of a spec in declaration order: own, MCP (per server tools, then resource tools),
/// knowledge, memory, built-in, skills, sub-agents, package tools, the self-build tool, then –
/// added by the caller with [`ToolNamespace::push`] – code pieces.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolNamespace {
    entries: Vec<ToolEntry>,
}

impl ToolNamespace {
    /// The namespace of `spec`. Packages without `tools` contribute nothing: their names are only
    /// known after resolving. Code pieces are not included (they need the graph catalogs); add
    /// them with [`push`](Self::push).
    pub fn of(spec: &AgentSpec) -> Self {
        let mut ns = Self::default();
        for (index, tool) in spec.tools.iter().enumerate() {
            ns.add(&tool.name, ToolOrigin::Own, format!("/tools/{index}/name"));
        }
        for (server_name, server) in &spec.mcp_servers {
            let base = format!("/mcp_servers/{}", escape_pointer_token(server_name));
            for (index, tool) in server.tools.iter().enumerate() {
                ns.add(tool, ToolOrigin::Mcp, format!("{base}/tools/{index}"));
            }
            for tool in server.resource_tools(server_name) {
                ns.add(tool, ToolOrigin::McpResource, format!("{base}/resources"));
            }
        }
        if let Some(knowledge) = spec.knowledge.as_ref().filter(|k| k.tool_enabled) {
            ns.add(&knowledge.tool, ToolOrigin::Knowledge, "/knowledge/tool");
        }
        if let Some(memory) = spec.long_term_memory.as_ref().filter(|m| m.tools) {
            ns.add(
                &memory.remember_tool,
                ToolOrigin::Memory,
                "/long_term_memory/remember_tool",
            );
            ns.add(
                &memory.recall_tool,
                ToolOrigin::Memory,
                "/long_term_memory/recall_tool",
            );
        }
        if let Some(builtin) = &spec.builtin_tools {
            for (index, tool) in builtin.tools.iter().enumerate() {
                ns.add(
                    tool,
                    ToolOrigin::Builtin,
                    format!("/builtin_tools/tools/{index}"),
                );
            }
        }
        if !spec.skills.is_empty() {
            for tool in BUILTIN_SKILL_TOOLS {
                ns.add(*tool, ToolOrigin::Skill, "/skills");
            }
        }
        for name in spec.agent_tools.keys() {
            ns.add(
                name,
                ToolOrigin::SubAgent,
                format!("/agent_tools/{}", escape_pointer_token(name)),
            );
        }
        for (index, package) in spec.tool_packages.iter().enumerate() {
            let base = format!("/tool_packages/{index}");
            for (tool_index, tool) in package.tools.iter().flatten().enumerate() {
                let pointer = if package.renames.contains_key(tool) {
                    format!("{base}/as/{}", escape_pointer_token(tool))
                } else {
                    format!("{base}/tools/{tool_index}")
                };
                ns.add(package.local_name(tool), ToolOrigin::Package, pointer);
            }
        }
        if let Some(self_tools) = &spec.self_tools {
            ns.add(&self_tools.tool, ToolOrigin::SelfBuild, "/self_tools/tool");
        }
        ns
    }

    fn add(&mut self, name: impl Into<String>, origin: ToolOrigin, pointer: impl Into<String>) {
        self.entries.push(ToolEntry::new(name, origin, pointer));
    }

    /// Appends a tool that the spec model cannot know by itself, e.g. a code piece.
    pub fn push(
        &mut self,
        name: impl Into<String>,
        origin: ToolOrigin,
        pointer: impl Into<String>,
    ) {
        self.add(name, origin, pointer);
    }

    /// All entries in declaration order.
    pub fn entries(&self) -> &[ToolEntry] {
        &self.entries
    }

    /// The names taken, in declaration order (a name that is declared twice appears twice).
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.name.as_str())
    }

    /// The first entry that has `name`.
    pub fn find(&self, name: &str) -> Option<&ToolEntry> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    /// `true` if some tool has `name`.
    pub fn is_taken(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// The names taken by everything except the entries of `origin`.
    pub fn names_except_origin(&self, origin: ToolOrigin) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .filter(move |entry| entry.origin != origin)
            .map(|entry| entry.name.as_str())
    }

    /// The names taken by everything except the entry declared at `pointer`.
    pub fn names_except_pointer<'a>(
        &'a self,
        pointer: &'a str,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.entries
            .iter()
            .filter(move |entry| entry.pointer != pointer)
            .map(|entry| entry.name.as_str())
    }

    /// Every entry whose name an earlier entry already has, with that earlier entry, in
    /// declaration order of the later one.
    pub fn duplicates(&self) -> Vec<ToolCollision<'_>> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(index, later)| {
                self.entries[..index]
                    .iter()
                    .find(|earlier| earlier.name == later.name)
                    .map(|earlier| ToolCollision { later, earlier })
            })
            .collect()
    }
}

impl AgentSpec {
    /// The tool namespace of this spec (ADR-0117), without code pieces.
    pub fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::of(self)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn spec(extra: serde_json::Value) -> AgentSpec {
        let mut doc = json!({
            "spec_version": 1,
            "name": "t",
            "model": { "provider": "p", "id": "m" },
            "system_prompt": "s",
            "graph": { "nodes": [], "edges": [] }
        });
        doc.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        AgentSpec::parse(&doc.to_string()).unwrap()
    }

    fn tool(name: &str) -> serde_json::Value {
        json!({ "name": name, "description": "d", "args": {}, "code": "Ok(String::new())" })
    }

    fn entry(ns: &ToolNamespace, name: &str) -> (ToolOrigin, String) {
        let found = ns.find(name).unwrap_or_else(|| panic!("{name} missing"));
        (found.origin, found.pointer.clone())
    }

    fn every_origin() -> AgentSpec {
        spec(json!({
            "tools": [tool("own")],
            "mcp_servers": { "docs": { "command": "x", "tools": ["mcp"], "resources": true } },
            "knowledge": { "sources": ["k"], "embedding_model": { "provider": "p", "id": "e" } },
            "long_term_memory": { "embedding_model": { "provider": "p", "id": "e" } },
            "builtin_tools": { "tools": ["think"] },
            "skills": [{ "dir": "s" }],
            "agent_tools": { "helper": { "url": "https://a.example" } },
            "tool_packages": [{
                "oci": "ghcr.io/a/b:1", "tools": ["pkg", "renamed"], "as": { "renamed": "fresh" }
            }],
            "self_tools": {}
        }))
    }

    #[test]
    fn lists_every_origin_with_its_pointer_in_declaration_order() {
        let ns = every_origin().tool_namespace();
        let listed: Vec<_> = ns
            .entries()
            .iter()
            .map(|e| (e.name.as_str(), e.origin, e.pointer.as_str()))
            .collect();
        assert_eq!(
            listed,
            [
                ("own", ToolOrigin::Own, "/tools/0/name"),
                ("mcp", ToolOrigin::Mcp, "/mcp_servers/docs/tools/0"),
                (
                    "docs_list_resources",
                    ToolOrigin::McpResource,
                    "/mcp_servers/docs/resources"
                ),
                (
                    "docs_read_resource",
                    ToolOrigin::McpResource,
                    "/mcp_servers/docs/resources"
                ),
                ("search_knowledge", ToolOrigin::Knowledge, "/knowledge/tool"),
                (
                    "remember",
                    ToolOrigin::Memory,
                    "/long_term_memory/remember_tool"
                ),
                (
                    "recall",
                    ToolOrigin::Memory,
                    "/long_term_memory/recall_tool"
                ),
                ("think", ToolOrigin::Builtin, "/builtin_tools/tools/0"),
                ("load_skill", ToolOrigin::Skill, "/skills"),
                ("read_skill_file", ToolOrigin::Skill, "/skills"),
                ("helper", ToolOrigin::SubAgent, "/agent_tools/helper"),
                ("pkg", ToolOrigin::Package, "/tool_packages/0/tools/0"),
                ("fresh", ToolOrigin::Package, "/tool_packages/0/as/renamed"),
                ("create_tool", ToolOrigin::SelfBuild, "/self_tools/tool"),
            ]
        );
    }

    #[test]
    fn leaves_out_what_the_spec_does_not_offer() {
        let ns = spec(json!({
            "knowledge": {
                "sources": ["k"], "tool_enabled": false, "inject": true,
                "embedding_model": { "provider": "p", "id": "e" }
            },
            "long_term_memory": {
                "tools": false, "auto_recall": true,
                "embedding_model": { "provider": "p", "id": "e" }
            },
            "tool_packages": [{ "oci": "ghcr.io/a/b:1" }]
        }))
        .tool_namespace();
        assert!(ns.entries().is_empty());
    }

    #[test]
    fn escapes_server_and_agent_names_in_pointers() {
        let ns = spec(json!({
            "agent_tools": { "a~b": { "url": "https://a.example" } }
        }))
        .tool_namespace();
        assert_eq!(entry(&ns, "a~b").1, "/agent_tools/a~0b");
    }

    #[test]
    fn names_taken_by_everything_except_an_origin_or_a_pointer() {
        let ns = every_origin().tool_namespace();
        assert!(ns.is_taken("own"));
        assert!(!ns.is_taken("nothing"));
        assert!(!ns.names_except_origin(ToolOrigin::Own).any(|n| n == "own"));
        assert!(
            ns.names_except_origin(ToolOrigin::Own)
                .any(|n| n == "think")
        );
        assert!(
            !ns.names_except_pointer("/builtin_tools/tools/0")
                .any(|n| n == "think")
        );
        assert!(
            ns.names_except_pointer("/builtin_tools/tools/0")
                .any(|n| n == "own")
        );
    }

    #[test]
    fn reports_a_duplicate_at_the_later_declaration_naming_the_earlier_one() {
        let ns = spec(json!({
            "tools": [tool("think")],
            "builtin_tools": { "tools": ["think"] },
        }))
        .tool_namespace();
        let duplicates = ns.duplicates();
        assert_eq!(duplicates.len(), 1);
        assert_eq!(duplicates[0].later.pointer, "/builtin_tools/tools/0");
        assert_eq!(duplicates[0].earlier.origin, ToolOrigin::Own);
        assert_eq!(duplicates[0].earlier.pointer, "/tools/0/name");
    }

    #[test]
    fn finds_a_duplicate_between_every_pair_of_origins() {
        let spec = every_origin();
        let base = spec.tool_namespace();
        let entries = base.entries().to_vec();
        for (i, first) in entries.iter().enumerate() {
            for second in &entries[i + 1..] {
                if first.origin == second.origin && first.pointer == second.pointer {
                    continue;
                }
                let mut ns = ToolNamespace::default();
                ns.push(&first.name, first.origin, &first.pointer);
                ns.push(&first.name, second.origin, &second.pointer);
                let duplicates = ns.duplicates();
                assert_eq!(duplicates.len(), 1, "{first:?} / {second:?}");
                assert_eq!(duplicates[0].later.origin, second.origin);
                assert_eq!(duplicates[0].earlier.origin, first.origin);
            }
        }
    }

    #[test]
    fn spec_declarations_with_the_same_name_collide_across_origins() {
        let ns = spec(json!({
            "tools": [tool("shared")],
            "mcp_servers": { "docs": { "command": "x", "tools": ["shared"] } },
            "agent_tools": { "shared": { "url": "https://a.example" } },
            "self_tools": { "tool": "shared" }
        }))
        .tool_namespace();
        let pointers: Vec<_> = ns
            .duplicates()
            .iter()
            .map(|d| d.later.pointer.clone())
            .collect();
        assert_eq!(
            pointers,
            [
                "/mcp_servers/docs/tools/0",
                "/agent_tools/shared",
                "/self_tools/tool"
            ]
        );
    }

    #[test]
    fn pushed_code_pieces_come_last() {
        let mut ns = every_origin().tool_namespace();
        ns.push(
            "code_main_x",
            ToolOrigin::CodePiece,
            "/graph/nodes/0/params/code",
        );
        let last = ns.entries().last().unwrap();
        assert_eq!(last.origin, ToolOrigin::CodePiece);
    }

    #[test]
    fn external_origins_are_those_that_external_tools_lists() {
        let spec = every_origin();
        let external: Vec<String> = spec
            .tool_namespace()
            .entries()
            .iter()
            .filter(|e| e.origin.is_external())
            .map(|e| e.name.clone())
            .collect();
        let mut listed = spec.external_tools();
        let mut expected = external;
        listed.sort();
        expected.sort();
        assert_eq!(listed, expected);
    }
}
