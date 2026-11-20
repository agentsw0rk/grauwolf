//! The file of `--consumers`: systems outside SPIRE that hold the trust domain (ADR-0120
//! decision 4) – ConfigMaps to rewrite and workloads to restart – and the replacement rule for
//! their texts.

use grauwolf_k8s::{Kind, kinds};

/// A ConfigMap to rewrite: all its keys or the listed ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigMapRef {
    pub namespace: String,
    pub name: String,
    pub keys: Option<Vec<String>>,
}

/// A workload to restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restart {
    pub kind: Kind,
    pub namespace: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Consumers {
    pub config_maps: Vec<ConfigMapRef>,
    pub restarts: Vec<Restart>,
}

/// Parses the TOML of `--consumers` (`[[configmap]]` and `[[restart]]` tables).
pub fn parse(text: &str) -> Result<Consumers, String> {
    let table: toml::Table = text
        .parse()
        .map_err(|error| format!("not valid TOML: {error}"))?;
    if let Some(unknown) = table
        .keys()
        .find(|key| !matches!(key.as_str(), "configmap" | "restart"))
    {
        return Err(format!(
            "unknown section '{unknown}' (only [[configmap]] and [[restart]])"
        ));
    }
    let mut consumers = Consumers::default();
    for (index, entry) in entries(&table, "configmap")?.into_iter().enumerate() {
        let at = format!("[[configmap]] #{}", index + 1);
        check_fields(entry, &["namespace", "name", "keys"], &at)?;
        let keys = match entry.get("keys") {
            None => None,
            Some(toml::Value::Array(keys)) => Some(
                keys.iter()
                    .map(|key| {
                        key.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| format!("{at}: keys must be strings"))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Some(_) => return Err(format!("{at}: keys must be a list of strings")),
        };
        consumers.config_maps.push(ConfigMapRef {
            namespace: field(entry, "namespace", &at)?,
            name: field(entry, "name", &at)?,
            keys,
        });
    }
    for (index, entry) in entries(&table, "restart")?.into_iter().enumerate() {
        let at = format!("[[restart]] #{}", index + 1);
        check_fields(entry, &["kind", "namespace", "name"], &at)?;
        let kind = match field(entry, "kind", &at)?.as_str() {
            "Deployment" => kinds::DEPLOYMENT,
            "StatefulSet" => kinds::STATEFUL_SET,
            "DaemonSet" => kinds::DAEMON_SET,
            other => {
                return Err(format!(
                    "{at}: kind '{other}' is not Deployment, StatefulSet or DaemonSet"
                ));
            }
        };
        consumers.restarts.push(Restart {
            kind,
            namespace: field(entry, "namespace", &at)?,
            name: field(entry, "name", &at)?,
        });
    }
    Ok(consumers)
}

fn entries<'a>(table: &'a toml::Table, name: &str) -> Result<Vec<&'a toml::Table>, String> {
    match table.get(name) {
        None => Ok(Vec::new()),
        Some(toml::Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_table()
                    .ok_or_else(|| format!("[[{name}]] entries must be tables"))
            })
            .collect(),
        Some(_) => Err(format!("'{name}' must be written as [[{name}]]")),
    }
}

fn check_fields(entry: &toml::Table, allowed: &[&str], at: &str) -> Result<(), String> {
    match entry.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(format!(
            "{at}: unknown field '{key}' (allowed: {})",
            allowed.join(", ")
        )),
        None => Ok(()),
    }
}

fn field(entry: &toml::Table, name: &str, at: &str) -> Result<String, String> {
    entry
        .get(name)
        .and_then(toml::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("{at}: '{name}' is missing"))
}

/// Replaces the trust domain `old` by `new` in the text of a consumer, and only where it is a
/// trust domain: `spiffe://<old>` followed by `/`, `"`, `'`, whitespace or the end, and the
/// quoted strings `"<old>"` and `'<old>'`. A bare occurrence (`foo.<old>`, `https://<old>/`)
/// stays.
pub fn replace_domain(text: &str, old: &str, new: &str) -> String {
    let id = format!("spiffe://{old}");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if let Some(tail) = rest.strip_prefix(&id)
            && tail
                .chars()
                .next()
                .is_none_or(|n| matches!(n, '/' | '"' | '\'') || n.is_whitespace())
        {
            out.push_str("spiffe://");
            out.push_str(new);
            rest = tail;
            continue;
        }
        if matches!(c, '"' | '\'')
            && let Some(tail) = rest[1..].strip_prefix(old)
            && tail.starts_with(c)
        {
            out.push(c);
            out.push_str(new);
            rest = tail;
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Lines (1-based, text) that still mention the `old` domain in any form after a change to `new`.
/// The new domain is left out of the search: it may contain the old one (`agentswork.de` ->
/// `test.agentswork.de`), and that is no leftover.
pub fn mentions(text: &str, old: &str, new: &str) -> Vec<(usize, String)> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| line.replace(new, "").contains(old))
        .map(|(index, line)| (index + 1, line.trim().to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_configmaps_and_restarts() {
        let consumers = parse(
            "[[configmap]]\nnamespace = \"scion\"\nname = \"scion-server\"\nkeys = [\"acl.toml\"]\n\n[[configmap]]\nnamespace = \"scion\"\nname = \"other\"\n\n[[restart]]\nkind = \"Deployment\"\nnamespace = \"scion\"\nname = \"scion-server\"\n",
        )
        .unwrap();
        assert_eq!(consumers.config_maps.len(), 2);
        assert_eq!(
            consumers.config_maps[0].keys.as_deref(),
            Some(&["acl.toml".to_owned()][..])
        );
        assert_eq!(consumers.config_maps[1].keys, None);
        assert_eq!(consumers.restarts[0].kind, kinds::DEPLOYMENT);
        assert_eq!(parse("").unwrap(), Consumers::default());
    }

    #[test]
    fn mistakes_in_the_file_are_named() {
        for (text, expected) in [
            ("[[configmap]]\nnamespace = \"a\"\n", "'name' is missing"),
            (
                "[[restart]]\nkind = \"Pod\"\nnamespace = \"a\"\nname = \"b\"\n",
                "kind 'Pod'",
            ),
            (
                "[[restart]]\nkind = \"Deployment\"\nnamespace = \"a\"\nname = \"b\"\nx = 1\n",
                "unknown field 'x'",
            ),
            ("[[config]]\n", "unknown section 'config'"),
            (
                "[[configmap]]\nnamespace = \"a\"\nname = \"b\"\nkeys = \"k\"\n",
                "keys must be a list",
            ),
            ("= broken", "not valid TOML"),
        ] {
            let error = parse(text).unwrap_err();
            assert!(error.contains(expected), "{text:?}: {error}");
        }
    }

    #[test]
    fn replaces_ids_and_quoted_domains_but_never_a_bare_substring() {
        let text = "acl = \"spiffe://example.org/ns/a/sa/b\"\ndomain = \"example.org\"\nalt: 'example.org'\nid spiffe://example.org\nid spiffe://example.org and more\nhost = \"foo.example.org\"\nurl = \"https://example.org/x\"\nlonger = \"spiffe://example.org.evil/x\"\nsub = \"spiffe://notexample.org/x\"\nbare example.org here\n";
        let expected = "acl = \"spiffe://agentswork.de/ns/a/sa/b\"\ndomain = \"agentswork.de\"\nalt: 'agentswork.de'\nid spiffe://agentswork.de\nid spiffe://agentswork.de and more\nhost = \"foo.example.org\"\nurl = \"https://example.org/x\"\nlonger = \"spiffe://example.org.evil/x\"\nsub = \"spiffe://notexample.org/x\"\nbare example.org here\n";
        assert_eq!(
            replace_domain(text, "example.org", "agentswork.de"),
            expected
        );
        assert_eq!(
            replace_domain("spiffe://example.org", "example.org", "n.de"),
            "spiffe://n.de"
        );
    }

    #[test]
    fn lists_the_lines_that_still_mention_a_domain() {
        assert_eq!(
            mentions(
                "a\nhost = foo.example.org\nb",
                "example.org",
                "agentswork.de"
            ),
            [(2, "host = foo.example.org".to_owned())]
        );
    }

    /// Found against the real cluster: with agentswork.de -> test.agentswork.de every rewritten
    /// line was reported as still mentioning the old domain, because the new one contains it.
    #[test]
    fn the_new_domain_does_not_count_as_a_mention_of_the_old_one() {
        let text = "trust_domain = \"test.agentswork.de\"\nid = \"spiffe://test.agentswork.de/ns/a/sa/a\"\nhost = foo.agentswork.de";
        assert_eq!(
            mentions(text, "agentswork.de", "test.agentswork.de"),
            [(3, "host = foo.agentswork.de".to_owned())]
        );
    }
}
