//! The `run_command` tool (ADR-0099). Needs a POSIX system.
#![cfg(unix)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use grauwolf_tools::{DynTool, ToolError};
use grauwolf_tools_command::{CommandConfig, command_tool};
use serde_json::{Value, json};

fn root(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("command-tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn path_env() -> Vec<(String, String)> {
    vec![("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into())]
}

fn config(allowed: &[&str]) -> CommandConfig {
    let mut config = CommandConfig::new(allowed.iter().map(|s| (*s).to_owned()).collect());
    config.env = path_env();
    config
}

fn tool(root: &std::path::Path, config: CommandConfig) -> std::sync::Arc<dyn DynTool> {
    command_tool(root, config).unwrap()
}

async fn run(tool: &dyn DynTool, args: Value) -> Result<String, ToolError> {
    tool.call_json(args).await
}

#[tokio::test]
async fn runs_an_allowed_program_and_reports_exit_code_and_output() {
    let dir = root("allowed");
    std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
    let tool = tool(&dir, config(&["cat", "ls"]));

    let result = run(&*tool, json!({ "program": "cat", "args": ["a.txt"] }))
        .await
        .unwrap();

    assert!(result.starts_with("exit code: 0"), "{result}");
    assert!(result.contains("hello"), "{result}");
}

#[tokio::test]
async fn reports_a_failing_exit_code_and_stderr() {
    let dir = root("exit-code");
    let tool = tool(&dir, config(&["ls"]));

    let result = run(&*tool, json!({ "program": "ls", "args": ["missing-file"] }))
        .await
        .unwrap();

    assert!(result.starts_with("exit code: "), "{result}");
    assert!(!result.starts_with("exit code: 0"), "{result}");
    assert!(result.contains("stderr:"), "{result}");
    assert!(result.contains("missing-file"), "{result}");
}

#[tokio::test]
async fn refuses_a_program_that_is_not_allowed() {
    let dir = root("refused");
    let tool = tool(&dir, config(&["ls"]));

    for program in ["cat", "/bin/ls", "../ls", "sh"] {
        let error = run(&*tool, json!({ "program": program }))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("not allowed"),
            "{program}: {error}"
        );
    }
}

#[tokio::test]
async fn an_absolute_path_runs_when_it_is_listed() {
    let dir = root("absolute");
    let Some(ls) = ["/bin/ls", "/usr/bin/ls"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
    else {
        return;
    };
    let tool = tool(&dir, config(&[ls]));

    let result = run(&*tool, json!({ "program": ls })).await.unwrap();

    assert!(result.starts_with("exit code: 0"), "{result}");
}

#[tokio::test]
async fn arguments_are_not_interpreted_by_a_shell() {
    let dir = root("no-shell");
    let tool = tool(&dir, config(&["echo"]));

    let result = run(
        &*tool,
        json!({ "program": "echo", "args": ["a; touch pwned", "$HOME", "*"] }),
    )
    .await
    .unwrap();

    assert!(result.contains("a; touch pwned $HOME *"), "{result}");
    assert!(!dir.join("pwned").exists());
}

#[tokio::test]
async fn the_working_directory_must_stay_inside_the_root() {
    let dir = root("cwd");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub").join("x.txt"), "").unwrap();
    let tool = tool(&dir, config(&["ls"]));

    let inside = run(&*tool, json!({ "program": "ls", "cwd": "sub" }))
        .await
        .unwrap();
    assert!(inside.contains("x.txt"), "{inside}");
    for cwd in ["..", "/", "sub/../..", "/tmp"] {
        let error = run(&*tool, json!({ "program": "ls", "cwd": cwd }))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("root"), "{cwd}: {error}");
    }
    std::os::unix::fs::symlink("/", dir.join("link")).unwrap();
    let error = run(&*tool, json!({ "program": "ls", "cwd": "link" }))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("root"), "{error}");
}

#[tokio::test]
async fn the_environment_is_cleared_except_the_listed_variables() {
    let dir = root("env");
    let mut config = config(&["env"]);
    config.env.push(("VISIBLE".into(), "yes".into()));
    let tool = tool(&dir, config);

    let result = run(&*tool, json!({ "program": "env" })).await.unwrap();

    assert!(result.contains("VISIBLE=yes"), "{result}");
    assert!(result.contains("PATH="), "{result}");
    assert!(!result.contains("CARGO"), "{result}");
    assert!(!result.contains("HOME="), "{result}");
}

#[tokio::test]
async fn stdin_is_closed() {
    let dir = root("stdin");
    let tool = tool(&dir, config(&["cat"]));

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        run(&*tool, json!({ "program": "cat" })),
    )
    .await
    .expect("cat must see end of input")
    .unwrap();

    assert!(result.starts_with("exit code: 0"), "{result}");
}

#[tokio::test]
async fn output_is_truncated_with_a_note() {
    let dir = root("truncate");
    std::fs::write(dir.join("big.txt"), "x".repeat(10_000)).unwrap();
    let mut config = config(&["cat"]);
    config.max_output_bytes = 100;
    let tool = tool(&dir, config);

    let result = run(&*tool, json!({ "program": "cat", "args": ["big.txt"] }))
        .await
        .unwrap();

    assert!(result.contains("truncated"), "{result}");
    assert!(result.len() < 600, "{}", result.len());
    assert!(result.contains(&"x".repeat(100)));
    assert!(!result.contains(&"x".repeat(101)));
}

#[tokio::test]
async fn a_timeout_kills_the_whole_process_group() {
    let dir = root("timeout");
    let mut config = CommandConfig::new(vec!["*".into()]);
    config.shell = true;
    config.env = path_env();
    config.timeout = Duration::from_secs(1);
    let tool = tool(&dir, config);
    let started = Instant::now();

    // The child of the shell writes a marker after 3 s unless it is killed.
    let result = run(
        &*tool,
        json!({ "command": "(sleep 3; touch survivor) & sleep 30" }),
    )
    .await
    .unwrap();

    assert!(result.contains("timed out after 1 s"), "{result}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!dir.join("survivor").exists(), "the grandchild survived");
}

#[tokio::test]
async fn shell_mode_runs_sh_c_with_pipes_in_the_root() {
    let dir = root("shell");
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    let mut config = CommandConfig::new(vec!["*".into()]);
    config.shell = true;
    config.env = path_env();
    let tool = tool(&dir, config);

    let result = run(
        &*tool,
        json!({ "command": "cat a.txt | wc -l && echo done" }),
    )
    .await
    .unwrap();

    assert!(result.starts_with("exit code: 0"), "{result}");
    assert!(result.contains('3') && result.contains("done"), "{result}");
}

#[tokio::test]
async fn the_descriptor_follows_the_mode() {
    let dir = root("descriptor");
    let allow = tool(&dir, config(&["ls", "wc"]));
    let mut shell_config = CommandConfig::new(vec!["*".into()]);
    shell_config.shell = true;
    let shell = tool(&dir, shell_config);

    assert_eq!(allow.name(), "run_command");
    let descriptor = allow.descriptor();
    assert!(descriptor.description.contains("ls, wc"));
    assert_eq!(descriptor.parameters["required"], json!(["program"]));
    assert_eq!(
        shell.descriptor().parameters["required"],
        json!(["command"])
    );
}

#[tokio::test]
async fn missing_arguments_are_invalid() {
    let dir = root("invalid");
    let tool = tool(&dir, config(&["ls"]));

    assert!(matches!(
        run(&*tool, json!({})).await,
        Err(ToolError::InvalidArgs { .. })
    ));
    assert!(matches!(
        run(&*tool, json!({ "program": "ls", "args": [1] })).await,
        Err(ToolError::InvalidArgs { .. })
    ));
}
