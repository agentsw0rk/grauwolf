//! Values from the environment in specs (`${VAR}` in headers of MCP servers and A2A agents).

use crate::config::{Env, ProcessEnv};

/// Replaces `${VAR}` with the value of the environment variable of the process; a missing
/// variable is an error.
pub fn expand(value: &str) -> Result<String, String> {
    expand_with(value, &ProcessEnv)
}

/// Replaces `${VAR}` with the value of the variable in `env`; a missing variable is an error.
pub fn expand_with(value: &str, env: &dyn Env) -> Result<String, String> {
    let mut expanded = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        let end = rest[start..]
            .find('}')
            .ok_or_else(|| format!("`${{` without `}}` in '{value}'"))?;
        let variable = &rest[start + 2..start + end];
        let replacement = env
            .var(variable)
            .ok_or_else(|| format!("missing environment variable {variable}"))?;
        expanded.push_str(&rest[..start]);
        expanded.push_str(&replacement);
        rest = &rest[start + end + 1..];
    }
    expanded.push_str(rest);
    Ok(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MapEnv;

    #[test]
    fn replaces_variables_from_the_given_environment() {
        let env = MapEnv::new([("TOKEN", "abc"), ("EMPTY", "")]);
        assert_eq!(
            expand_with("Bearer ${TOKEN}${EMPTY}!", &env).unwrap(),
            "Bearer abc!"
        );
        assert_eq!(expand_with("plain", &env).unwrap(), "plain");
    }

    #[test]
    fn a_missing_variable_and_an_open_brace_are_errors() {
        let env = MapEnv::default();
        assert_eq!(
            expand_with("${NOPE}", &env).unwrap_err(),
            "missing environment variable NOPE"
        );
        assert_eq!(
            expand_with("a ${b", &env).unwrap_err(),
            "`${` without `}` in 'a ${b'"
        );
    }
}
