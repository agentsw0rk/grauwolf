//! What commands that run an agent need besides the provisioned agent: approval, input parsing and
//! the description (the memory, key-value store and tools directory come from provisioning,
//! ADR-0114).

use std::sync::Arc;

use grauwolf_agents::events::ToolDecision;
use grauwolf_interpreter::ToolApprover;
use grauwolf_prompt::ToolCall;
use grauwolf_spec::AgentSpec;
use serde_json::Value;

/// Approver on the console for the calls the spec makes approval-required: tools the agent
/// wants to build itself (`self_tools` with `confirm`, ADR-0075) and every call of the command
/// tool in shell mode (ADR-0099). It shows what is about to happen on stderr and reads
/// `j`/`ja`/`y`/`yes`.
pub fn console_approver(spec: &AgentSpec) -> Option<ToolApprover> {
    let builds = spec.self_tools.as_ref().is_some_and(|tools| tools.confirm);
    let commands = spec
        .builtin_tools
        .as_ref()
        .and_then(|builtin| builtin.command.as_ref())
        .is_some_and(|command| command.needs_approval());
    if !builds && !commands {
        return None;
    }
    Some(Arc::new(|call: &ToolCall| {
        let call = call.clone();
        Box::pin(async move {
            let question = approval_question(&call);
            let confirmed = tokio::task::spawn_blocking(move || {
                eprint!("{question}");
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer).is_ok()
                    && matches!(
                        answer.trim().to_lowercase().as_str(),
                        "j" | "ja" | "y" | "yes"
                    )
            })
            .await
            .unwrap_or(false);
            if confirmed {
                ToolDecision::Proceed
            } else {
                ToolDecision::Deny(refusal_text(&call))
            }
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = ToolDecision> + Send>>
    }))
}

/// What the console shows before it asks.
fn approval_question(call: &ToolCall) -> String {
    if call.tool == "run_command" {
        return format!(
            "The agent wants to run the shell command:\n---\n{}\n---\nRun? [y/N] ",
            call.args["command"].as_str().unwrap_or("?")
        );
    }
    format!(
        "The agent wants to build the tool '{}': {}\n---\n{}\n---\nBuild? [y/N] ",
        call.args["name"].as_str().unwrap_or("?"),
        call.args["description"].as_str().unwrap_or(""),
        call.args["code"].as_str().unwrap_or(""),
    )
}

/// The tool result when the user says no.
fn refusal_text(call: &ToolCall) -> String {
    if call.tool == "run_command" {
        return "The user refused to run the command.".to_owned();
    }
    format!(
        "The user refused to build the tool '{}'.",
        call.args["name"].as_str().unwrap_or("?")
    )
}

/// Input as a value: text for `input: string`, otherwise JSON according to the input type.
pub fn parse_input(spec: &AgentSpec, input: String) -> Result<Value, String> {
    if spec.input == grauwolf_spec::STRING_TYPE {
        return Ok(Value::String(input));
    }
    serde_json::from_str(&input)
        .map_err(|error| format!("input is not JSON for type {}: {error}", spec.input))
}

/// Description of an agent for callers: system prompt or default text.
pub fn agent_description(spec: &AgentSpec) -> String {
    spec.system_prompt
        .clone()
        .unwrap_or_else(|| format!("grauwolf agent {}", spec.name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use grauwolf_spec::AgentSpecExt;

    fn spec(command: serde_json::Value) -> AgentSpec {
        AgentSpec::from_json(
            &serde_json::json!({
                "spec_version": 1, "name": "runner",
                "model": { "provider": "ollama", "id": "m" },
                "builtin_tools": { "tools": ["run_command"], "root": ".", "command": command },
                "strategy": { "kind": "single_run" }
            })
            .to_string(),
        )
        .unwrap()
    }

    #[test]
    fn the_console_asks_for_shell_commands_only() {
        let shell =
            serde_json::json!({ "allowed": ["*"], "shell": true, "i_understand_shell": true });
        let mut open = shell.clone();
        open["approve_each_call"] = serde_json::json!(false);

        assert!(console_approver(&spec(shell)).is_some());
        assert!(console_approver(&spec(open)).is_none());
        assert!(console_approver(&spec(serde_json::json!({ "allowed": ["ls"] }))).is_none());
    }

    #[test]
    fn the_question_shows_the_command_line() {
        let call = ToolCall {
            id: "1".into(),
            tool: "run_command".into(),
            args: serde_json::json!({ "command": "rm -rf build" }),
        };

        assert!(approval_question(&call).contains("rm -rf build"));
        assert_eq!(refusal_text(&call), "The user refused to run the command.");
    }
}
