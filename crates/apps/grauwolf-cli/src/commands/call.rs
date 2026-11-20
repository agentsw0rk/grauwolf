//! `grauwolf call <url>` (ADR-0111 stage 3): calls an agent's `/run` once and prints its output –
//! the command of a schedule's CronJob. With `SPIFFE_ENDPOINT_SOCKET` it authenticates with the
//! workload identity (`https://`: mTLS, the server must be `--spiffe-id`; `http://`: a JWT-SVID for
//! the audience `--spiffe-id`), else with `--token` or not at all. `--grant` sends the person's
//! grant (`Grauwolf-Grant`).
//!
//! Exit codes: 0 the run answered; 1 the call failed (unreachable, the agent's error, any other
//! HTTP error); 3 refused (401 or 403: the caller or the grant is not accepted).

use std::io::Read;
use std::process::ExitCode;
use std::time::Duration;

use grauwolf_host::config::ProcessEnv;
use grauwolf_host::identity::call_agent;
use serde_json::Value;

use crate::cli::CallArgs;
use crate::output::{Io, errln, outln};

/// The exit code of a refused call (401, 403).
pub const REFUSED: u8 = 3;

/// `/run` appended unless the URL already names a path.
fn run_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    let path_start = trimmed
        .find("://")
        .map(|scheme| scheme + 3)
        .and_then(|after| trimmed[after..].find('/').map(|slash| after + slash));
    match path_start {
        Some(_) => trimmed.to_owned(),
        None => format!("{trimmed}/run"),
    }
}

/// Runs `grauwolf call`.
pub fn run(io: &Io, args: CallArgs) -> ExitCode {
    match call(io, args) {
        Ok(code) => code,
        Err(error) => {
            errln!(io, "error: {error}");
            ExitCode::from(1)
        }
    }
}

fn input(args: &CallArgs) -> Result<Value, String> {
    let text = match &args.input {
        Some(text) => text.clone(),
        None => {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|error| format!("stdin: {error}"))?;
            text
        }
    };
    if args.json {
        serde_json::from_str(&text)
            .map_err(|error| format!("--json: the input is not JSON: {error}"))
    } else {
        Ok(Value::String(text))
    }
}

fn call(io: &Io, args: CallArgs) -> Result<ExitCode, String> {
    let url = run_url(&args.url);
    let input = input(&args)?;
    let runtime = crate::errors::runtime()?;
    let answer = runtime.block_on(async {
        let credential = grauwolf_host::identity::caller_credential(
            &ProcessEnv,
            &url,
            args.spiffe_id.as_deref(),
            args.token.as_deref(),
        )
        .await?;
        call_agent(
            &url,
            &credential,
            args.grant.as_deref(),
            input,
            Duration::from_secs(args.timeout),
        )
        .await
    })?;
    let output = &answer.body["output"];
    if answer.status == 200 && !output.is_null() {
        match output {
            Value::String(text) => outln!(io, "{text}"),
            other => outln!(
                io,
                "{}",
                serde_json::to_string_pretty(other).unwrap_or_default()
            ),
        }
        return Ok(ExitCode::SUCCESS);
    }
    let reason = answer.body["error"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| answer.body.to_string());
    errln!(io, "error: {url} answered HTTP {}: {reason}", answer.status);
    Ok(match answer.status {
        401 | 403 => ExitCode::from(REFUSED),
        _ => ExitCode::from(1),
    })
}

#[cfg(test)]
mod tests {
    use super::run_url;

    #[test]
    fn run_is_appended_to_a_bare_address() {
        assert_eq!(run_url("https://a.b.svc:8443"), "https://a.b.svc:8443/run");
        assert_eq!(run_url("https://a.b.svc:8443/"), "https://a.b.svc:8443/run");
        assert_eq!(run_url("http://h:8080/run"), "http://h:8080/run");
        assert_eq!(run_url("http://h:8080/api/run"), "http://h:8080/api/run");
    }
}
