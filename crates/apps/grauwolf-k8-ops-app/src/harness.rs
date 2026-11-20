//! The harness file of ADR-0128 as the input of `agent deploy --harness` (decision 6).

use std::path::Path;

use grauwolf_spec::Harness;

/// Reads a harness file; errors name the file and the field.
pub fn read(file: &Path) -> Result<Harness, String> {
    let json =
        std::fs::read_to_string(file).map_err(|error| format!("{}: {error}", file.display()))?;
    Harness::parse_named(&file.display().to_string(), &json)
}
