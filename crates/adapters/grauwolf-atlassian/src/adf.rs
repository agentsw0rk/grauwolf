//! Atlassian Document Format for plain texts (ADR-0104).

use serde_json::{Value, json};

/// Lines of `text` split at `\n`, `\r\n` and `\r`; a trailing line break yields a last empty
/// line, the empty text one empty line.
fn lines(text: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\n' => {
                result.push(&text[start..index]);
                start = index + 1;
            }
            b'\r' => {
                result.push(&text[start..index]);
                if bytes.get(index + 1) == Some(&b'\n') {
                    index += 1;
                }
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    result.push(&text[start..]);
    result
}

/// `text` as an ADF document: one paragraph per line; ADF forbids empty text nodes, so blank
/// lines become empty paragraphs.
pub fn document(text: &str) -> Value {
    let paragraphs: Vec<Value> = lines(text)
        .into_iter()
        .map(|line| {
            if line.trim().is_empty() {
                json!({ "type": "paragraph" })
            } else {
                json!({ "type": "paragraph", "content": [{ "type": "text", "text": line }] })
            }
        })
        .collect();
    json!({ "type": "doc", "version": 1, "content": paragraphs })
}
