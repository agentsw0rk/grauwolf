//! The price table (ADR-0071): read from a file, for the CLI (`--prices`) and for generated
//! agents (`GRAUWOLF_PRICES`, ADR-0119).

use std::path::Path;

use grauwolf_prompt::Prices;

/// Reads the price table `{ "currency", "models": { "<provider>:<id>": { "input", "output", … } } }`
/// from `path`; errors name the file.
pub fn load(path: &Path) -> Result<Prices, String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|error| format!("{}: invalid price table: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, content: &str) -> (grauwolf_test::TestDir, std::path::PathBuf) {
        let dir = grauwolf_test::TestDir::new("prices");
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        (dir, path)
    }

    #[test]
    fn reads_a_price_table() {
        let (_dir, path) = file(
            "ok.json",
            r#"{ "currency": "EUR", "models": { "ollama:m": { "input": 1.0, "output": 2.0 } } }"#,
        );
        let prices = load(&path).unwrap();
        assert_eq!(prices.currency, "EUR");
        assert!(prices.models.contains_key("ollama:m"));
    }

    #[test]
    fn errors_name_the_file() {
        let (_dir, broken) = file("broken.json", "{ nope");
        let error = load(&broken).unwrap_err();
        assert!(
            error.contains("broken.json: invalid price table"),
            "{error}"
        );
        let missing = load(&broken.with_file_name("missing.json")).unwrap_err();
        assert!(missing.contains("missing.json"), "{missing}");
    }
}
