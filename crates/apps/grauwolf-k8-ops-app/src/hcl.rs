//! The configuration files of SPIRE, read without an HCL library: the trust domain, plugin
//! blocks and string assignments (ADR-0120), and the one rewrite `trust-domain set` makes: the
//! value of the `trust_domain` assignment, nothing else on the line.

/// `trust_domain = "example.org"` of an HCL file (the first assignment outside comments).
pub fn trust_domain_of_hcl(text: &str) -> Option<String> {
    string_assignment(text, "trust_domain").filter(|value| !value.is_empty())
}

/// The value of the first `key = "value"` outside comments (also inside a one-line block).
pub fn string_assignment(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = strip_comment(line);
        line.match_indices(key).find_map(|(at, _)| {
            let before = line[..at].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.') {
                return None;
            }
            let rest = line[at + key.len()..].trim_start().strip_prefix('=')?;
            let rest = rest.trim_start().strip_prefix('"')?;
            Some(rest[..rest.find('"')?].to_owned())
        })
    })
}

/// The inside of the first block `<kind> "<name>" { ... }` (comments removed), as in
/// `DataStore "sql" { plugin_data { ... } }`.
pub fn block(text: &str, kind: &str, name: &str) -> Option<String> {
    let clean: String = text
        .lines()
        .map(strip_comment)
        .collect::<Vec<_>>()
        .join("\n");
    let header = clean.lines().scan(0usize, |offset, line| {
        let start = *offset;
        *offset += line.len() + 1;
        Some((start, line))
    });
    for (start, line) in header {
        let Some(rest) = line.trim().strip_prefix(kind) else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix(&format!("\"{name}\"")) else {
            continue;
        };
        if !rest.trim_start().starts_with('{') {
            continue;
        }
        let open = start + line.find('{')?;
        return matching_brace(&clean[open..]).map(|inside| inside.to_owned());
    }
    None
}

/// The text between the `{` that starts `text` and its matching `}`.
fn matching_brace(text: &str) -> Option<&str> {
    let (mut depth, mut in_string) = (0usize, false);
    for (index, byte) in text.bytes().enumerate() {
        match byte {
            b'"' => in_string = !in_string,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[1..index]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Does the configuration set up federation (a `federation { ... }` block or `federates_with`)?
pub fn has_federation(text: &str) -> bool {
    text.lines().any(|line| {
        let line = strip_comment(line).trim();
        line.contains("federates_with")
            || line
                .strip_prefix("federation")
                .is_some_and(|rest| rest.trim_start().starts_with('{'))
    })
}

/// How a configuration file writes the trust domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    /// `trust_domain = "example.org"`.
    Hcl,
    /// `trustDomain: example.org`.
    Yaml,
}

/// One changed line (1-based number).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineChange {
    pub line: usize,
    pub old: String,
    pub new: String,
}

/// A text with the trust domain replaced, and what changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewrite {
    pub text: String,
    pub changes: Vec<LineChange>,
}

/// Replaces `old` by `new` in every `trust_domain` / `trustDomain` assignment whose value is
/// exactly `old`; everything else on the line (indentation, comment, quotes) stays.
pub fn rewrite_trust_domain(text: &str, old: &str, new: &str, syntax: Syntax) -> Rewrite {
    let mut changes = Vec::new();
    let mut lines = Vec::new();
    for (index, line) in text.split('\n').enumerate() {
        let rewritten = match syntax {
            Syntax::Hcl => rewrite_hcl_line(line, old, new),
            Syntax::Yaml => rewrite_yaml_line(line, old, new),
        };
        match rewritten {
            Some(new_line) => {
                changes.push(LineChange {
                    line: index + 1,
                    old: line.trim().to_owned(),
                    new: new_line.trim().to_owned(),
                });
                lines.push(new_line);
            }
            None => lines.push(line.to_owned()),
        }
    }
    Rewrite {
        text: lines.join("\n"),
        changes,
    }
}

fn rewrite_hcl_line(line: &str, old: &str, new: &str) -> Option<String> {
    let code = strip_comment(line);
    let rest = code.trim_start().strip_prefix("trust_domain")?;
    let value = rest
        .trim_start()
        .strip_prefix('=')?
        .trim_start()
        .strip_prefix('"')?;
    let after = value.strip_prefix(old)?;
    after
        .starts_with('"')
        .then(|| splice(line, code.len() - value.len(), old, new))
}

fn rewrite_yaml_line(line: &str, old: &str, new: &str) -> Option<String> {
    let code = line.split('#').next().unwrap_or("");
    let rest = code.trim_start().strip_prefix("trustDomain:")?.trim_start();
    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''));
    let value = quote.map_or(rest, |q| &rest[q.len_utf8()..]);
    let after = value.strip_prefix(old)?;
    let closing = match quote {
        Some(q) => after.strip_prefix(q)?,
        None => after,
    };
    closing
        .trim()
        .is_empty()
        .then(|| splice(line, code.len() - value.len(), old, new))
}

/// `line` with `old` (which starts at byte `at`) replaced by `new`.
fn splice(line: &str, at: usize, old: &str, new: &str) -> String {
    format!("{}{new}{}", &line[..at], &line[at + old.len()..])
}

/// `trustDomain: example.org` of the YAML of spire-controller-manager.
pub fn trust_domain_of_yaml(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.split('#').next().unwrap_or("").trim();
        let rest = line.strip_prefix("trustDomain:")?.trim();
        let value = rest.trim_matches(|c| c == '"' || c == '\'');
        (!value.is_empty()).then(|| value.to_owned())
    })
}

/// A line without a `#` or `//` comment (outside of a quoted string).
fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let bytes = line.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        match byte {
            b'"' => in_string = !in_string,
            b'#' if !in_string => return &line[..index],
            b'/' if !in_string && bytes.get(index + 1) == Some(&b'/') => return &line[..index],
            _ => {}
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER_CONF: &str = r#"
server {
    bind_address = "0.0.0.0"
    # trust_domain = "old.example"
    trust_domain = "agentswork.de"   # the domain
    data_dir = "/run/spire/data"
}
plugins {}
"#;

    #[test]
    fn reads_the_assignment_and_skips_comments() {
        assert_eq!(
            trust_domain_of_hcl(SERVER_CONF).as_deref(),
            Some("agentswork.de")
        );
    }

    #[test]
    fn agent_conf_with_the_assignment_inside_a_block_works_and_a_missing_one_is_none() {
        assert_eq!(
            trust_domain_of_hcl("workload_api {\n  trust_domain = \"x.org\"\n}").as_deref(),
            Some("x.org")
        );
        assert_eq!(trust_domain_of_hcl("server { }"), None);
        assert_eq!(trust_domain_of_hcl("trust_domain = \"\""), None);
        assert_eq!(trust_domain_of_hcl("// trust_domain = \"x\""), None);
        assert_eq!(trust_domain_of_hcl("trust_domain_other = \"x\""), None);
    }

    #[test]
    fn yaml_of_the_controller_manager() {
        let yaml = "apiVersion: spire.spiffe.io/v1alpha1\nkind: ControllerManagerConfig\ntrustDomain: agentswork.de\nignoreNamespaces:\n  - spire-system\n";
        assert_eq!(trust_domain_of_yaml(yaml).as_deref(), Some("agentswork.de"));
        assert_eq!(
            trust_domain_of_yaml("trustDomain: \"q.org\" # c").as_deref(),
            Some("q.org")
        );
        assert_eq!(trust_domain_of_yaml("kind: X"), None);
    }

    #[test]
    fn rewrites_only_the_assignment_with_the_old_value() {
        let text = "server {\n    trust_domain = \"old.org\"   # the domain\n    jwt_issuer = \"https://oidc.old.org\"\n}\nagent {\n  trust_domain = \"other.org\"\n  // trust_domain = \"old.org\"\n}";
        let rewrite = rewrite_trust_domain(text, "old.org", "new.de", Syntax::Hcl);
        assert_eq!(
            rewrite.changes,
            [LineChange {
                line: 2,
                old: "trust_domain = \"old.org\"   # the domain".into(),
                new: "trust_domain = \"new.de\"   # the domain".into()
            }]
        );
        assert_eq!(
            rewrite.text,
            text.replace(
                "    trust_domain = \"old.org\"",
                "    trust_domain = \"new.de\""
            )
        );
        // A domain that merely starts with the old one is not it.
        let longer = rewrite_trust_domain(
            "trust_domain = \"old.org.evil\"",
            "old.org",
            "n",
            Syntax::Hcl,
        );
        assert!(longer.changes.is_empty());
    }

    #[test]
    fn rewrites_the_yaml_value_with_or_without_quotes() {
        for (line, expected) in [
            ("trustDomain: old.org", "trustDomain: new.de"),
            (
                "  trustDomain: \"old.org\" # c",
                "  trustDomain: \"new.de\" # c",
            ),
            ("trustDomain: 'old.org'", "trustDomain: 'new.de'"),
        ] {
            let rewrite = rewrite_trust_domain(line, "old.org", "new.de", Syntax::Yaml);
            assert_eq!(rewrite.text, expected);
            assert_eq!(rewrite.changes.len(), 1);
        }
        assert!(
            rewrite_trust_domain("trustDomain: old.org.x", "old.org", "n", Syntax::Yaml)
                .changes
                .is_empty()
        );
        assert!(
            rewrite_trust_domain("name: old.org", "old.org", "n", Syntax::Yaml)
                .changes
                .is_empty()
        );
    }

    #[test]
    fn finds_plugin_blocks_and_their_assignments() {
        let conf = "plugins {\n  DataStore \"sql\" {\n    plugin_data {\n      database_type = \"sqlite3\"\n      connection_string = \"/run/spire/data/datastore.sqlite3\" # file\n    }\n  }\n  # KeyManager \"memory\" { plugin_data {} }\n  KeyManager \"disk\" { plugin_data { keys_path = \"/run/spire/data/keys.json\" } }\n}";
        let store = block(conf, "DataStore", "sql").unwrap();
        assert_eq!(
            string_assignment(&store, "database_type").as_deref(),
            Some("sqlite3")
        );
        assert_eq!(
            string_assignment(&store, "connection_string").as_deref(),
            Some("/run/spire/data/datastore.sqlite3")
        );
        let keys = block(conf, "KeyManager", "disk").unwrap();
        assert_eq!(
            string_assignment(&keys, "keys_path").as_deref(),
            Some("/run/spire/data/keys.json")
        );
        assert!(
            block(conf, "KeyManager", "memory").is_none(),
            "a comment is not a block"
        );
        assert!(block(conf, "Notifier", "k8sbundle").is_none());
    }

    #[test]
    fn detects_federation_outside_comments() {
        assert!(has_federation(
            "server {\n  federation {\n    bundle_endpoint {}\n  }\n}"
        ));
        assert!(has_federation("federates_with \"other.org\" {}"));
        assert!(!has_federation(
            "# federation {\n// federates_with x\nfederation_enabled = true"
        ));
    }
}
