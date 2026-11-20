//! HTML to readable text for knowledge pages (ADR-0113).
//!
//! Drops everything that is page furniture (`head`, `script`, `style`, `noscript`, `svg`, `template`,
//! `nav`, `header`, `footer`, `aside`, `form`, comments), keeps the structure as plain lines:
//! headings as `#` lines, paragraphs separated by a blank line, list items as `- `, table cells
//! separated by ` | `.

use regex::Regex;

/// Elements dropped together with their content.
const DROPPED: &[&str] = &[
    "head", "script", "style", "noscript", "svg", "template", "nav", "header", "footer", "aside",
    "form",
];

/// Readable text of `html`; paragraphs are separated by one blank line.
pub fn html_to_text(html: &str) -> String {
    let mut text = Regex::new(r"(?s)<!--.*?-->")
        .expect("valid regex")
        .replace_all(html, " ")
        .into_owned();
    for tag in DROPPED {
        let element = Regex::new(&format!(r"(?is)<{tag}\b[^>]*>.*?</{tag}\s*>")).expect("regex");
        text = element.replace_all(&text, " ").into_owned();
    }
    // Line breaks in the source are insignificant; only the tags below make lines.
    let text = text.replace(['\n', '\r'], " ");
    let heading = Regex::new(r"(?i)<h([1-6])\b[^>]*>").expect("valid regex");
    let text = heading.replace_all(&text, |caps: &regex::Captures| {
        let level: usize = caps[1].parse().unwrap_or(1);
        format!("\n\n{} ", "#".repeat(level))
    });
    let replacements = [
        (r"(?i)</h[1-6]\s*>", "\n\n"),
        (r"(?i)<li\b[^>]*>", "\n- "),
        (r"(?i)</t[dh]\s*>", " | "),
        (r"(?i)</tr\s*>", "\n"),
        (
            r"(?i)</?(p|div|section|article|main|ul|ol|table|blockquote|pre|figure|dl|dd|dt)\b[^>]*>",
            "\n\n",
        ),
        (r"(?i)<br\b[^>]*>", "\n"),
        (r"(?s)<[^>]*>", ""),
    ];
    let mut text = text.into_owned();
    for (pattern, with) in replacements {
        text = Regex::new(pattern)
            .expect("valid regex")
            .replace_all(&text, with)
            .into_owned();
    }
    let text = decode_entities(&text);
    let mut out: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        let line = line.trim_end_matches(" |").trim_end_matches('|').trim();
        if line.is_empty() && out.last().is_none_or(String::is_empty) {
            continue;
        }
        out.push(line.to_owned());
    }
    out.join("\n").trim().to_owned()
}

/// Decodes the common named entities and numeric ones (`&#233;`, `&#xE9;`).
fn decode_entities(text: &str) -> String {
    let entity = Regex::new(r"&(#[0-9]+|#[xX][0-9a-fA-F]+|[a-zA-Z]+);").expect("valid regex");
    entity
        .replace_all(text, |caps: &regex::Captures| {
            let name = &caps[1];
            let decoded = if let Some(hex) = name.strip_prefix("#x").or(name.strip_prefix("#X")) {
                u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
            } else if let Some(number) = name.strip_prefix('#') {
                number.parse().ok().and_then(char::from_u32)
            } else {
                match name {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some(' '),
                    "ndash" => Some('\u{2013}'),
                    "mdash" => Some('\u{2014}'),
                    "hellip" => Some('\u{2026}'),
                    "copy" => Some('\u{a9}'),
                    "auml" => Some('\u{e4}'),
                    "ouml" => Some('\u{f6}'),
                    "uuml" => Some('\u{fc}'),
                    "szlig" => Some('\u{df}'),
                    _ => None,
                }
            };
            decoded.map_or_else(|| caps[0].to_owned(), String::from)
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::html_to_text;

    const PAGE: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Docs</title>
<style>body { color: red }</style>
<script>window.dataLayer = []; if (a < b) { track("x") }</script></head>
<body>
<header><nav><a href="/">Home</a> <a href="/docs">Docs</a></nav></header>
<main>
<h1 class="title">House &amp; Garden</h1>
<p>Quiet hours start at <strong>22:00</strong>.<br>Please be   considerate.</p>
<h2>Rules</h2>
<ul><li>No <em>loud</em> music</li><li>Keep doors closed</li></ul>
<table><tr><th>Day</th><th>Hours</th></tr>
<tr><td>Mon</td><td>8&ndash;20</td></tr></table>
<!-- a comment with <p>markup</p> -->
<form action="/s"><input name="q"><button>Search</button></form>
<p>Caf&eacute; &#233; &#xE9; &unknown;</p>
</main>
<aside>Related links</aside>
<footer>&copy; 2026 Example Corp</footer>
<noscript>Please enable JavaScript</noscript>
</body></html>"#;

    #[test]
    fn keeps_headings_paragraphs_lists_and_table_cells() {
        let text = html_to_text(PAGE);

        assert_eq!(
            text,
            "# House & Garden\n\n\
             Quiet hours start at 22:00.\nPlease be considerate.\n\n\
             ## Rules\n\n\
             - No loud music\n- Keep doors closed\n\n\
             Day | Hours\nMon | 8\u{2013}20\n\n\
             Caf&eacute; \u{e9} \u{e9} &unknown;"
        );
    }

    #[test]
    fn drops_scripts_styles_navigation_forms_and_footers() {
        let text = html_to_text(PAGE);

        for noise in [
            "dataLayer",
            "color: red",
            "Home",
            "Search",
            "Related links",
            "Example Corp",
            "JavaScript",
            "comment",
        ] {
            assert!(!text.contains(noise), "{noise} in {text}");
        }
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(html_to_text("just text"), "just text");
    }
}
