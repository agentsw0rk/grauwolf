//! `grauwolf identity grant create | list | revoke` (ADR-0111 stage 3): a person's grants for
//! unattended runs. `create` runs a separate device code sign-in with `offline_access`, so the
//! grant gets a token family of its own (the login cache is not touched); the refresh token goes
//! to the identity service once and is not kept here. The person's cached login
//! (`grauwolf login`) authorizes every call.

use std::process::ExitCode;

use grauwolf_host::identity::{GrantRequest, GrantsClient};
use serde_json::json;

use crate::cli::{GrantCommand, GrantCreateArgs, GrantTarget};
use crate::commands::login;
use crate::output::{Io, outln, report};

/// Runs a `grant` subcommand.
pub fn run(io: &Io, command: GrantCommand) -> ExitCode {
    match command {
        GrantCommand::Create(args) => report(io, create(io, args)),
        GrantCommand::List { target, json } => report(io, list(io, target, json)),
        GrantCommand::Revoke { target, id } => report(io, revoke(io, target, &id)),
    }
}

/// The access token of the login and the identity service's URL: `--identity-url`, else
/// `GRAUWOLF_LOGIN_IDENTITY_URL`, else the one stored by `grauwolf login --identity-url`.
fn signed_in(target: &GrantTarget) -> Result<(String, GrantsClient, login::Login), String> {
    let login = login::current(target.config_dir.clone())?;
    let url = target
        .identity_url
        .clone()
        .or_else(|| std::env::var("GRAUWOLF_LOGIN_IDENTITY_URL").ok())
        .filter(|url| !url.trim().is_empty())
        .or_else(|| login.identity_url.clone())
        .ok_or(
            "no identity service: --identity-url, GRAUWOLF_LOGIN_IDENTITY_URL or \
             `grauwolf login --identity-url`",
        )?;
    let client = GrantsClient::new(&url)?;
    Ok((login.access_token.clone(), client, login))
}

/// `30d`, `4h`, `90m`, `45s` (a plain number is seconds) in seconds.
pub fn parse_duration(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let (number, unit) = match text.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((at, _)) => text.split_at(at),
        None => (text, "s"),
    };
    let factor = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => {
            return Err(format!(
                "'{text}' is not a duration such as 30d, 4h, 90m or 45s"
            ));
        }
    };
    number
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .map(|number| number * factor)
        .ok_or_else(|| format!("'{text}' is not a duration such as 30d, 4h, 90m or 45s"))
}

/// `2026-12-16 09:30 UTC` for Unix seconds.
pub fn date(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Days to civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60
    )
}

fn hours(seconds: u64) -> String {
    match seconds {
        s if s % 3600 == 0 => format!("{} h", s / 3600),
        s if s % 60 == 0 => format!("{} min", s / 60),
        s => format!("{s} s"),
    }
}

fn create(io: &Io, args: GrantCreateArgs) -> Result<(), String> {
    let (access_token, client, login) = signed_in(&args.target)?;
    let valid = parse_duration(&args.valid)?;
    let max_run = parse_duration(&args.max_run)?;
    let request = GrantRequest {
        agent: args.agent,
        trigger: args.trigger,
        purposes: args.purposes,
        valid_until: Some(login::now() + valid),
        max_run: Some(max_run),
    };
    let device = login.grant_login()?;
    let runtime = crate::errors::runtime()?;
    let grant = runtime.block_on(async {
        let code = device.start().await?;
        outln!(
            io,
            "The grant needs a sign-in of its own (it keeps that sign-in's refresh token, not your login's)."
        );
        match &code.message {
            Some(message) => outln!(io, "{message}"),
            None => outln!(
                io,
                "To sign in, open {} in a browser and enter the code {}.",
                code.verification_uri,
                code.user_code
            ),
        }
        outln!(io, "waiting for the sign-in (at most {} s) ...", code.expires_in);
        let tokens = device.wait(&code).await?;
        let refresh_token = tokens.refresh_token.ok_or(
            "the sign-in returned no refresh token: the client must be allowed offline_access",
        )?;
        let grant = client
            .create(&access_token, &request, &refresh_token)
            .await
            .map_err(|error| format!("the identity service refused the grant: {error}"))?;
        // The refresh token is not kept here.
        drop(refresh_token);
        Ok::<_, String>(grant)
    })?;
    outln!(io, "grant {} created", grant.id);
    outln!(io, "  agent:     {}", grant.agent);
    outln!(io, "  trigger:   {}", grant.trigger);
    if !grant.purposes.is_empty() {
        outln!(io, "  purposes:  {}", grant.purposes.join(", "));
    }
    outln!(io, "  valid to:  {}", date(grant.valid_until));
    outln!(io, "  max run:   {}", hours(grant.max_run));
    Ok(())
}

fn list(io: &Io, target: GrantTarget, as_json: bool) -> Result<(), String> {
    let (access_token, client, _) = signed_in(&target)?;
    let grants = crate::errors::runtime()?
        .block_on(client.list(&access_token))
        .map_err(|error| format!("the identity service refused the list: {error}"))?;
    if as_json {
        outln!(
            io,
            "{}",
            serde_json::to_string_pretty(&json!({ "grants": grants })).unwrap_or_default()
        );
        return Ok(());
    }
    if grants.is_empty() {
        outln!(io, "no grants");
    }
    for grant in grants {
        let purposes = match grant.purposes.is_empty() {
            true => "-".to_owned(),
            false => grant.purposes.join(","),
        };
        let used = grant.last_used.map(date).unwrap_or_else(|| "never".into());
        outln!(
            io,
            "{}  agent {}  trigger {}  purposes {}  valid to {}  max run {}  last used {}",
            grant.id,
            grant.agent,
            grant.trigger,
            purposes,
            date(grant.valid_until),
            hours(grant.max_run),
            used
        );
    }
    Ok(())
}

fn revoke(io: &Io, target: GrantTarget, id: &str) -> Result<(), String> {
    let (access_token, client, _) = signed_in(&target)?;
    crate::errors::runtime()?
        .block_on(client.revoke(&access_token, id))
        .map_err(|error| format!("grant {id} not revoked: {error}"))?;
    outln!(
        io,
        "grant {id} revoked (metadata and refresh token deleted)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_dates() {
        assert_eq!(parse_duration("30d").unwrap(), 30 * 86_400);
        assert_eq!(parse_duration("4h").unwrap(), 4 * 3600);
        assert_eq!(parse_duration("90m").unwrap(), 5400);
        assert_eq!(parse_duration("45").unwrap(), 45);
        for bad in ["", "0h", "4w", "h", "-1d"] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
        assert_eq!(date(0), "1970-01-01 00:00 UTC");
        assert_eq!(date(1_798_675_200 + 3600 + 120), "2026-12-31 01:02 UTC");
        assert_eq!(hours(4 * 3600), "4 h");
        assert_eq!(hours(90 * 60), "90 min");
    }
}
