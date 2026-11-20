use std::io::{BufRead, Write};

/// Asks for a secret without echo.
pub type Prompt<'a> = &'a dyn Fn(&str) -> std::io::Result<String>;

/// Where a command prints and reads: output, errors, stdin and (on a terminal) a hidden prompt.
pub struct Io<'a> {
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
    stdin: &'a mut dyn BufRead,
    prompt: Option<Prompt<'a>>,
}

impl<'a> Io<'a> {
    pub fn new(out: &'a mut dyn Write, err: &'a mut dyn Write, stdin: &'a mut dyn BufRead) -> Self {
        Self {
            out,
            err,
            stdin,
            prompt: None,
        }
    }

    /// A terminal: secrets and confirmations are asked for with `prompt` (which does not echo).
    pub fn with_hidden_prompt(
        mut self,
        prompt: &'a dyn Fn(&str) -> std::io::Result<String>,
    ) -> Self {
        self.prompt = Some(prompt);
        self
    }

    pub fn line(&mut self, text: &str) {
        let _ = writeln!(self.out, "{}", text.trim_end());
    }

    /// `text` exactly as it is (a chunk of a log).
    pub fn text(&mut self, text: &str) {
        let _ = write!(self.out, "{text}");
        let _ = self.out.flush();
    }

    /// `text` and a newline, exactly as it is (a secret value is not trimmed).
    pub fn value(&mut self, text: &str) {
        let _ = writeln!(self.out, "{text}");
    }

    pub fn error(&mut self, text: &str) {
        let _ = writeln!(self.err, "{}", text.trim_end());
    }

    /// A line of stdin (a confirmation); `None` at the end of the input.
    pub fn read_line(&mut self) -> Option<String> {
        let mut line = String::new();
        match self.stdin.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim().to_owned()),
        }
    }

    /// Asks `Proceed? [y/N]` and reads the answer from stdin; without input the answer is no.
    pub fn confirm(&mut self) -> bool {
        self.line("Proceed? [y/N]");
        let answer = self.read_line().unwrap_or_default();
        matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes")
    }

    /// A secret: from the hidden prompt on a terminal, else all of stdin (one trailing newline
    /// removed). Never echoed.
    pub fn read_secret(&mut self, label: &str) -> Result<String, String> {
        let value = match self.prompt {
            Some(prompt) => {
                prompt(label).map_err(|error| format!("cannot read the value: {error}"))?
            }
            None => {
                let mut text = String::new();
                std::io::Read::read_to_string(&mut self.stdin, &mut text)
                    .map_err(|_| "the value on stdin is not valid UTF-8 text".to_owned())?;
                text.strip_suffix('\n')
                    .map(|rest| rest.strip_suffix('\r').unwrap_or(rest))
                    .unwrap_or(&text)
                    .to_owned()
            }
        };
        if value.is_empty() {
            return Err("the value is empty".to_owned());
        }
        Ok(value)
    }
}
