//! `grauwolf spec egress` over the example specs of the user guide, and `app/egress.json` of
//! `spec generate --dockerfile`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use grauwolf_cli::{Captured, Io, run};
use serde_json::Value;

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../docs/user-guide/examples")
}

fn grauwolf(args: &[&str]) -> (ExitCode, Captured) {
    let (io, captured) = Io::captured();
    let code = run(std::iter::once("grauwolf").chain(args.iter().copied()), &io);
    (code, captured)
}

fn egress(file: &Path, extra: &[&str]) -> Vec<Value> {
    let mut args = vec!["spec", "egress", file.to_str().unwrap(), "--format", "json"];
    args.extend(extra);
    let (code, output) = grauwolf(&args);
    assert_eq!(
        code,
        ExitCode::SUCCESS,
        "{}: {}",
        file.display(),
        output.stderr()
    );
    serde_json::from_str::<Vec<Value>>(&output.stdout())
        .unwrap_or_else(|error| panic!("{}: {error}", file.display()))
}

fn example(name: &str) -> Vec<Value> {
    egress(&examples().join(name), &[])
}

fn hosts(destinations: &[Value]) -> Vec<String> {
    destinations
        .iter()
        .filter_map(|d| d["host"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn every_example_has_a_deterministic_well_formed_list() {
    let mut count = 0;
    for entry in std::fs::read_dir(examples()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let first = egress(&path, &[]);
        assert_eq!(first, egress(&path, &[]), "{}", path.display());
        for destination in &first {
            for key in [
                "scheme", "host", "port", "purpose", "pointer", "phase", "from_env",
            ] {
                assert!(destination.get(key).is_some(), "{}: {key}", path.display());
            }
            assert!(["run", "build"].contains(&destination["phase"].as_str().unwrap()));
            if let Some(host) = destination["host"].as_str() {
                assert_eq!(host, host.to_lowercase());
            }
            // Never a guess: no host means the address comes from a variable.
            assert!(
                !destination["host"].is_null() || destination["from_env"].is_string(),
                "{}: {destination}",
                path.display()
            );
        }
        let again = first.clone();
        let mut sorted = first;
        sorted.sort_by_key(|d| (d["phase"] == "build") as u8);
        assert_eq!(
            again,
            sorted,
            "run entries come first in {}",
            path.display()
        );
        count += 1;
    }
    assert!(count > 40, "only {count} examples");
}

#[test]
fn the_hello_example_reaches_only_ollama() {
    let all = example("01-hello.json");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0]["host"], "localhost");
    assert_eq!(all[0]["port"], 11434);
    assert_eq!(all[0]["from_env"], "OLLAMA_HOST");
    assert_eq!(all[0]["pointer"], "/model/provider");
}

#[test]
fn providers_are_listed_with_their_default_hosts() {
    assert_eq!(hosts(&example("02-openai.json")), ["api.openai.com"]);
    assert_eq!(hosts(&example("02-anthropic.json")), ["api.anthropic.com"]);
    assert_eq!(
        hosts(&example("02-gemini.json")),
        ["generativelanguage.googleapis.com"]
    );
    let bedrock = example("02-bedrock.json");
    assert_eq!(bedrock[0]["from_env"], "AWS_REGION");
    assert!(
        bedrock[0]["host"]
            .as_str()
            .unwrap()
            .starts_with("bedrock-runtime.")
    );
}

#[test]
fn sub_agents_of_the_tree_are_included_with_their_spec_file() {
    let all = example("08-coordinator.json");
    assert!(
        all.iter()
            .any(|d| d["pointer"].as_str().unwrap().contains(".json:/")),
        "{all:#?}"
    );
}

#[test]
fn the_text_format_has_one_line_per_destination() {
    let file = examples().join("08-mcp-http.json");
    let json = egress(&file, &[]);
    let (code, output) = grauwolf(&["spec", "egress", file.to_str().unwrap()]);
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(output.stdout().lines().count(), json.len());
    assert!(output.stdout().starts_with("run "), "{}", output.stdout());
}

#[test]
fn otel_adds_the_otlp_variable() {
    let file = examples().join("01-hello.json");
    let all = egress(&file, &["--otel"]);
    assert!(
        all.iter()
            .any(|d| d["from_env"] == "OTEL_EXPORTER_OTLP_ENDPOINT")
    );
}

#[test]
fn a_grauwolf_toml_next_to_the_spec_names_openbao() {
    let dir = grauwolf_test::TestDir::new("egress-openbao");
    std::fs::copy(examples().join("01-hello.json"), dir.join("hello.json")).unwrap();
    std::fs::write(
        dir.join("grauwolf.toml"),
        "[credentials.openbao]\naddress = \"http://openbao.openbao.svc:8200\"\n",
    )
    .unwrap();
    let all = egress(&dir.join("hello.json"), &[]);
    assert!(
        hosts(&all).contains(&"openbao.openbao.svc".to_owned()),
        "{all:#?}"
    );
}

#[test]
fn generate_dockerfile_writes_the_same_list_to_app_egress_json() {
    let dir = grauwolf_test::TestDir::new("egress-image");
    let spec = dir.join("agent.json");
    std::fs::write(
        &spec,
        serde_json::json!({"spec_version": 1, "name": "agent",
            "model": {"provider": "openai", "id": "m"},
            "builtin_tools": {"tools": ["fetch"], "fetch": {"allowed_hosts": ["api.example.com"]}},
            "strategy": {"kind": "single_run"}})
        .to_string(),
    )
    .unwrap();
    let out = dir.join("out");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["spec", "generate", spec.to_str().unwrap(), "--out"])
        .arg(&out)
        .arg("--dockerfile")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let written: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(out.join("app/egress.json")).unwrap())
            .unwrap();
    assert_eq!(written, egress(&spec, &[]));
    assert_eq!(hosts(&written), ["api.example.com", "api.openai.com"]);
}
