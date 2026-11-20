//! The built-in tool groups (ADR-0046, ADR-0098, ADR-0104, ADR-0117): one row per group with its
//! tools, the section of `builtin_tools` it needs and whether it needs `root`. The spec checks,
//! the host's dispatch and the generator read this table. Cargo features are not part of it
//! (ADR-0016): the generator and the host map a [`BuiltinGroup`] to their features themselves.

use crate::{
    BUILTIN_COMMAND_TOOLS, BUILTIN_CONFLUENCE_TOOLS, BUILTIN_FETCH_TOOLS, BUILTIN_FILE_TOOLS,
    BUILTIN_GIT_TOOLS, BUILTIN_JIRA_TOOLS, BUILTIN_KV_TOOLS, BUILTIN_THINK_TOOLS,
    BUILTIN_TODO_TOOLS, BUILTIN_USER_TOOLS, BuiltinToolsSpec,
};

/// A group of built-in tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinGroup {
    /// `say_to_user`, `ask_user`.
    User,
    /// `think`.
    Think,
    /// `fetch` (section `fetch`).
    Fetch,
    /// `run_command` (section `command`, needs `root`).
    Command,
    /// `add_task`, `complete_task`, `remove_task`, `list_tasks`.
    Todo,
    /// `store_value`, `retrieve_value`, `list_keys` (needs `root`).
    Kv,
    /// The `jira_*` tools (section `jira`).
    Jira,
    /// The `confluence_*` tools (section `confluence`).
    Confluence,
    /// The `git_*` tools (section `git`).
    Git,
    /// The file tools (need `root`).
    File,
}

/// One row of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinGroupInfo {
    /// The group.
    pub group: BuiltinGroup,
    /// The tools of the group.
    pub tools: &'static [&'static str],
    /// The section of `builtin_tools` that the group needs, if any.
    pub section: Option<&'static str>,
    /// `true` if the group needs `builtin_tools.root`.
    pub needs_root: bool,
}

const fn row(
    group: BuiltinGroup,
    tools: &'static [&'static str],
    section: Option<&'static str>,
    needs_root: bool,
) -> BuiltinGroupInfo {
    BuiltinGroupInfo {
        group,
        tools,
        section,
        needs_root,
    }
}

/// All built-in tool groups. The order is the order in which the host builds them.
pub const BUILTIN_GROUPS: &[BuiltinGroupInfo] = &[
    row(BuiltinGroup::User, BUILTIN_USER_TOOLS, None, false),
    row(BuiltinGroup::Think, BUILTIN_THINK_TOOLS, None, false),
    row(
        BuiltinGroup::Fetch,
        BUILTIN_FETCH_TOOLS,
        Some("fetch"),
        false,
    ),
    row(
        BuiltinGroup::Command,
        BUILTIN_COMMAND_TOOLS,
        Some("command"),
        true,
    ),
    row(BuiltinGroup::Todo, BUILTIN_TODO_TOOLS, None, false),
    row(BuiltinGroup::Kv, BUILTIN_KV_TOOLS, None, true),
    row(BuiltinGroup::Jira, BUILTIN_JIRA_TOOLS, Some("jira"), false),
    row(
        BuiltinGroup::Confluence,
        BUILTIN_CONFLUENCE_TOOLS,
        Some("confluence"),
        false,
    ),
    row(BuiltinGroup::Git, BUILTIN_GIT_TOOLS, Some("git"), false),
    row(BuiltinGroup::File, BUILTIN_FILE_TOOLS, None, true),
];

impl BuiltinGroup {
    /// The table row of the group.
    pub fn info(self) -> &'static BuiltinGroupInfo {
        BUILTIN_GROUPS
            .iter()
            .find(|info| info.group == self)
            .expect("every group has a row in BUILTIN_GROUPS")
    }

    /// The group of the built-in tool `name`; `None` for an unknown tool.
    pub fn of_tool(name: &str) -> Option<Self> {
        BUILTIN_GROUPS
            .iter()
            .find(|info| info.tools.contains(&name))
            .map(|info| info.group)
    }
}

impl BuiltinToolsSpec {
    /// The configured tools that belong to `group`, in the order of the spec.
    pub fn tools_of(&self, group: BuiltinGroup) -> Vec<&str> {
        self.tools
            .iter()
            .map(String::as_str)
            .filter(|name| group.info().tools.contains(name))
            .collect()
    }

    /// The groups that the configured tools belong to, in table order, each once.
    pub fn groups(&self) -> Vec<BuiltinGroup> {
        BUILTIN_GROUPS
            .iter()
            .map(|info| info.group)
            .filter(|group| !self.tools_of(*group).is_empty())
            .collect()
    }

    /// `true` if the section `section` (`jira`, `confluence`, `git`, `fetch`, `command`) is set.
    pub fn has_section(&self, section: &str) -> bool {
        match section {
            "jira" => self.jira.is_some(),
            "confluence" => self.confluence.is_some(),
            "git" => self.git.is_some(),
            "fetch" => self.fetch.is_some(),
            "command" => self.command.is_some(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn every_builtin_tool_belongs_to_exactly_one_group() {
        let all: Vec<&str> = BUILTIN_GROUPS
            .iter()
            .flat_map(|info| info.tools.iter().copied())
            .collect();
        let unique: BTreeSet<&str> = all.iter().copied().collect();
        assert_eq!(all.len(), unique.len(), "a tool is in two groups: {all:?}");
        for tool in &all {
            let owners = BUILTIN_GROUPS
                .iter()
                .filter(|info| info.tools.contains(tool))
                .count();
            assert_eq!(owners, 1, "{tool}");
        }
    }

    #[test]
    fn every_group_has_exactly_one_row_and_tools() {
        for info in BUILTIN_GROUPS {
            assert!(!info.tools.is_empty(), "{:?}", info.group);
            assert_eq!(
                BUILTIN_GROUPS
                    .iter()
                    .filter(|i| i.group == info.group)
                    .count(),
                1,
                "{:?}",
                info.group
            );
            assert_eq!(info.group.info(), info);
        }
    }

    #[test]
    fn the_sections_and_root_rules_of_the_groups() {
        let section = |group: BuiltinGroup| group.info().section;
        assert_eq!(section(BuiltinGroup::Jira), Some("jira"));
        assert_eq!(section(BuiltinGroup::Confluence), Some("confluence"));
        assert_eq!(section(BuiltinGroup::Git), Some("git"));
        assert_eq!(section(BuiltinGroup::Fetch), Some("fetch"));
        assert_eq!(section(BuiltinGroup::Command), Some("command"));
        assert_eq!(
            BUILTIN_GROUPS
                .iter()
                .filter(|i| i.section.is_some())
                .count(),
            5
        );
        let needs_root: Vec<_> = BUILTIN_GROUPS
            .iter()
            .filter(|info| info.needs_root)
            .map(|info| info.group)
            .collect();
        assert_eq!(
            needs_root,
            [BuiltinGroup::Command, BuiltinGroup::Kv, BuiltinGroup::File]
        );
    }

    #[test]
    fn every_section_of_the_table_is_a_section_of_the_spec() {
        let spec: BuiltinToolsSpec = serde_json::from_value(serde_json::json!({
            "tools": [],
            "jira": { "base_url": "https://a.example" },
            "confluence": { "base_url": "https://a.example" },
            "git": { "repo_url": "https://a.example/r.git" },
            "fetch": { "allowed_hosts": ["a.example"] },
            "command": { "allowed": ["ls"] }
        }))
        .unwrap();
        for info in BUILTIN_GROUPS {
            if let Some(section) = info.section {
                assert!(spec.has_section(section), "{section}");
            }
        }
        assert!(!BuiltinToolsSpec::default().has_section("jira"));
    }

    #[test]
    fn finds_the_group_of_a_tool() {
        assert_eq!(BuiltinGroup::of_tool("read_file"), Some(BuiltinGroup::File));
        assert_eq!(BuiltinGroup::of_tool("git_push"), Some(BuiltinGroup::Git));
        assert_eq!(BuiltinGroup::of_tool("shell"), None);
    }

    #[test]
    fn lists_the_tools_and_groups_a_spec_uses() {
        let spec = BuiltinToolsSpec {
            tools: ["git_push", "think", "say_to_user", "git_commit", "bogus"]
                .map(String::from)
                .to_vec(),
            ..BuiltinToolsSpec::default()
        };
        assert_eq!(spec.tools_of(BuiltinGroup::Git), ["git_push", "git_commit"]);
        assert_eq!(
            spec.groups(),
            [BuiltinGroup::User, BuiltinGroup::Think, BuiltinGroup::Git]
        );
    }
}
