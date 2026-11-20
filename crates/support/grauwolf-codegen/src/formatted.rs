//! grauwolf extension of the fork: syntax check with `syn` and formatting with
//! `prettyplease` (ADR-0013). Not part of upstream codegen 0.3.0.

use std::fmt;

use crate::Scope;

/// Error when emitting formatted code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// The generated code is not valid Rust.
    Syntax {
        /// Message from the parser.
        message: String,
        /// Line (1-based) in the unformatted code.
        line: usize,
        /// Column (0-based) in the unformatted code.
        column: usize,
    },
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::Syntax {
                message,
                line,
                column,
            } => write!(
                f,
                "invalid Rust code at line {line}, column {column}: {message}"
            ),
        }
    }
}

impl std::error::Error for FormatError {}

impl Scope {
    /// Emits the code, checked and formatted.
    ///
    /// The code is parsed with `syn`; if it is valid, `prettyplease` formats it.
    /// Ordinary comments (`//`) are lost in the process, doc comments are kept.
    pub fn to_formatted_string(&self) -> Result<String, FormatError> {
        let code = self.to_string();
        let file = syn::parse_file(&code).map_err(|error| {
            let start = error.span().start();
            FormatError::Syntax {
                message: error.to_string(),
                line: start.line,
                column: start.column,
            }
        })?;
        Ok(prettyplease::unparse(&file))
    }
}
