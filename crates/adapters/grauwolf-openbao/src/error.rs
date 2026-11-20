use std::fmt;

use serde_json::Value;

/// A failed call. The message holds the HTTP status and the server's `errors`, never a value or
/// a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaoError {
    /// The HTTP status, `None` for a transport error or an unexpected answer.
    pub status: Option<u16>,
    pub message: String,
}

impl BaoError {
    pub(crate) fn other(message: impl Into<String>) -> Self {
        Self {
            status: None,
            message: message.into(),
        }
    }

    /// `read: HTTP 403: permission denied` – the server's `errors` messages, which never hold
    /// secrets.
    pub(crate) fn status(what: &str, status: u16, body: &str) -> Self {
        let messages = serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|body| {
                body.get("errors").and_then(Value::as_array).map(|errors| {
                    errors
                        .iter()
                        .filter_map(Value::as_str)
                        .flat_map(plain_messages)
                        .collect::<Vec<_>>()
                        .join("; ")
                })
            })
            .filter(|messages| !messages.is_empty());
        let message = match messages {
            Some(messages) => format!("{what}: HTTP {status}: {messages}"),
            None => format!("{what}: HTTP {status}"),
        };
        Self {
            status: Some(status),
            message,
        }
    }
}

/// OpenBao wraps its messages as `"1 error occurred:\n\t* permission denied\n\n"`: drop the
/// heading and the bullets so the message stays one line (`permission denied`).
fn plain_messages(message: &str) -> Vec<String> {
    let lines: Vec<&str> = message
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let wrapped = lines
        .first()
        .is_some_and(|first| first.ends_with("occurred:") && first.contains("error"));
    if !wrapped {
        return vec![lines.join(" ")];
    }
    lines[1..]
        .iter()
        .map(|line| line.trim_start_matches('*').trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect()
}

impl fmt::Display for BaoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for BaoError {}

#[cfg(test)]
mod tests {
    use super::BaoError;

    /// What OpenBao 2.7 really answers for a policy that does not allow the path (found in the
    /// live check): one error string with a heading, a bullet and line breaks.
    #[test]
    fn a_wrapped_multi_line_error_becomes_one_plain_line() {
        let body = r#"{"errors":["1 error occurred:\n\t* permission denied\n\n"]}"#;
        assert_eq!(
            BaoError::status("read", 403, body).to_string(),
            "read: HTTP 403: permission denied"
        );
    }

    #[test]
    fn several_wrapped_errors_are_joined() {
        let body = r#"{"errors":["2 errors occurred:\n\t* first\n\t* second\n\n"]}"#;
        assert_eq!(
            BaoError::status("write", 400, body).to_string(),
            "write: HTTP 400: first; second"
        );
    }
}
