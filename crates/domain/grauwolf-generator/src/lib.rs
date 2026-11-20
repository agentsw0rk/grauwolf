//! Generator (E13, F13.6–F13.9): generates a Rust crate from an AgentSpec.
//!
//! The generated code uses the typed builder from `grauwolf-graph` and is therefore
//! checked at compile time (ADR-0017).

use grauwolf_spec::AgentSpecExt;
use std::collections::BTreeMap;
use std::path::PathBuf;

use grauwolf_codegen::{Block, Scope};

mod docker;
pub mod templates;
mod types;
pub use docker::{
    APP_DIR, Companion, CompanionKind, DockerfileOptions, companion_paths, companions,
    default_builder_image,
};
use grauwolf_spec::{
    AgentSpec, CODE_KIND, CodePiece, DEFAULT_PLAN_ITERATIONS, EdgeSpec, FINISH, FOR_EACH_KIND,
    ForEachBody, ForEachOnError, ForEachParams, GOAP_KIND, GraphSpec, ON_CODE_KIND, PARALLEL_KIND,
    PLAN_KIND, START, SUBGRAPH_KIND, SubgraphSpec, ValidationError,
};
use grauwolf_tools::ToolArtifact;
use serde_json::Value;

/// Error during generation.
#[derive(Debug, thiserror::Error)]
pub enum GeneratorError {
    /// The spec is invalid; contains all errors found.
    #[error("invalid spec: {}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    Invalid(Vec<ValidationError>),
    /// The generated code is not valid Rust (an error in the generator or in code from the spec).
    #[error("generated code is invalid: {0}")]
    InvalidCode(#[from] grauwolf_codegen::FormatError),
    /// The spec imports tool packages, but their tools were not passed in `GenerateOptions`.
    #[error(
        "the spec has `tool_packages`: resolve them first (`grauwolf spec generate` does) and \
         pass the tools in `GenerateOptions::package_tools`"
    )]
    UnresolvedPackages,
    /// No built artifact was passed for a tool of the spec.
    #[error("no artifact for tool '{tool}' – build the tools first (`grauwolf spec build`)")]
    MissingArtifact { tool: String },
    /// `tool_sources` has no guest crate for a tool of the spec.
    #[error("no tool source for tool '{tool}' – write its guest crate first")]
    MissingToolSource { tool: String },
    /// A type from `types` cannot be mapped to Rust.
    #[error("{path}: type not supported: {reason}")]
    UnsupportedType { path: String, reason: String },
    /// The spec uses something the generator cannot yet translate into code.
    #[error(
        "'{feature}' is not yet supported by the generator – run the spec with `grauwolf spec run`"
    )]
    Unsupported { feature: String },
    /// There is no generator template for a kind from the spec.
    #[error("no generator template for '{kind}'")]
    MissingTemplate { kind: String },
}

/// Where the generated crate gets the grauwolf crates from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrauwolfDependency {
    /// Local checkout: path to the workspace's `crates/` directory.
    /// (A variant for published versions follows with the first release.)
    Path(PathBuf),
}

/// A generated crate as files in memory; the caller writes them out (ADR-0016).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedCrate {
    /// Relative path → text content.
    pub files: BTreeMap<PathBuf, String>,
    /// Relative path → binary content (tool artifacts under `tools/`).
    pub binary_files: BTreeMap<PathBuf, Vec<u8>>,
}

/// What else the generated crate contains besides the library.
#[derive(Debug, Clone, Default)]
pub struct GenerateOptions {
    /// The tools of the spec's `tool_packages`, resolved and verified (ADR-0107). They are
    /// embedded like the tools of the spec; the generated crate does not know the packages.
    pub package_tools: Vec<grauwolf_tools::ResolvedTool>,
    /// An HTTP service: the binary `agent` (`src/main.rs`) behind the Cargo feature `service`
    /// (ADR-0093).
    pub service: bool,
    /// Depend on the facade with all its (default) features, whatever the spec needs, so that
    /// the dependency graph and the feature sets are the same for every spec. A builder image
    /// can then pre-compile the dependencies once (ADR-0096).
    pub full_facade: bool,
    /// Also a `Dockerfile` (and `.dockerignore`) that builds the service in the builder image;
    /// implies `service` (ADR-0096).
    pub dockerfile: Option<DockerfileOptions>,
    /// Tool code as sources instead of built modules (ADR-0095): per tool the guest crate
    /// `tools/<name>/` and the manifest `tools/tools.json` are written, and `build.rs` fails the
    /// build with a hint at `grauwolf tools build` while a module is missing. The library embeds
    /// `tools/<name>.wasm` as always. Every tool of the spec needs an entry; no artifacts are used.
    pub tool_sources: Option<Vec<grauwolf_tools::ToolSourceCrate>>,
}

/// Generates a standalone crate (`Cargo.toml`, `src/lib.rs`) for `spec`.
///
/// The tools of the spec come as prebuilt `artifacts` (ADR-0024); they are stored under
/// `tools/<name>.wasm` and executed by the generated crate in the sandbox.
pub fn generate_crate(
    spec: &AgentSpec,
    grauwolf: &GrauwolfDependency,
    artifacts: &[ToolArtifact],
) -> Result<GeneratedCrate, GeneratorError> {
    generate_crate_with(spec, grauwolf, artifacts, &GenerateOptions::default())
}

/// Like [`generate_crate`]; `options` add the service binary.
pub fn generate_crate_with(
    spec: &AgentSpec,
    grauwolf: &GrauwolfDependency,
    artifacts: &[ToolArtifact],
    options: &GenerateOptions,
) -> Result<GeneratedCrate, GeneratorError> {
    let service = options.service || options.dockerfile.is_some();
    // Package tools become tools of the spec, with their modules as artifacts: the crate then
    // embeds them like local tools and carries no reference to a registry.
    let (derived, all_artifacts);
    let (spec, artifacts) = if spec.tool_packages.is_empty() && options.package_tools.is_empty() {
        (spec, artifacts)
    } else if options.package_tools.is_empty() {
        return Err(GeneratorError::UnresolvedPackages);
    } else {
        derived = with_package_tools(spec, &options.package_tools);
        all_artifacts = artifacts
            .iter()
            .cloned()
            .chain(
                options
                    .package_tools
                    .iter()
                    .map(|tool| tool.artifact.clone()),
            )
            .collect::<Vec<_>>();
        (&derived, all_artifacts.as_slice())
    };
    let limits: BTreeMap<String, grauwolf_tools::ToolLimits> = options
        .package_tools
        .iter()
        .filter(|tool| !tool.limits.is_empty())
        .map(|tool| (tool.name.clone(), tool.limits))
        .collect();
    let lib = generate_lib_limited(spec, &limits)?;
    let mut files = BTreeMap::new();
    files.insert(
        PathBuf::from("Cargo.toml"),
        manifest(spec, grauwolf, options, service),
    );
    files.insert(PathBuf::from("src/lib.rs"), lib);
    if service {
        files.insert(PathBuf::from("src/main.rs"), service_main(spec));
    }
    if let Some(dockerfile) = &options.dockerfile {
        files.insert(
            PathBuf::from("Dockerfile"),
            docker::dockerfile(
                spec,
                dockerfile,
                options
                    .tool_sources
                    .as_ref()
                    .is_some_and(|tools| !tools.is_empty()),
            ),
        );
        files.insert(
            PathBuf::from(".dockerignore"),
            docker::dockerignore().to_owned(),
        );
    }
    let mut binary_files = BTreeMap::new();
    if let Some(sources) = &options.tool_sources {
        let mut used = Vec::new();
        for tool in &spec.tools {
            let source = sources
                .iter()
                .find(|source| source.name == tool.name)
                .ok_or_else(|| GeneratorError::MissingToolSource {
                    tool: tool.name.clone(),
                })?;
            files.insert(
                PathBuf::from(format!("tools/{}/Cargo.toml", tool.name)),
                source.manifest.clone(),
            );
            files.insert(
                PathBuf::from(format!("tools/{}/src/lib.rs", tool.name)),
                source.source.clone(),
            );
            used.push(source.clone());
        }
        // The code of the graphs is built like the tools (ADR-0102).
        for piece in spec.code_pieces() {
            let source = sources
                .iter()
                .find(|source| source.name == piece.name)
                .ok_or_else(|| GeneratorError::MissingToolSource {
                    tool: piece.name.clone(),
                })?;
            files.insert(
                PathBuf::from(format!("tools/{}/Cargo.toml", piece.name)),
                source.manifest.clone(),
            );
            files.insert(
                PathBuf::from(format!("tools/{}/src/lib.rs", piece.name)),
                source.source.clone(),
            );
            used.push(source.clone());
        }
        if !used.is_empty() {
            files.insert(
                PathBuf::from(grauwolf_tools::TOOL_SOURCES_FILE),
                grauwolf_tools::ToolSourcesManifest::for_crates(&used).to_json(),
            );
            files.insert(PathBuf::from("build.rs"), build_script(&used));
        }
        return Ok(GeneratedCrate {
            files,
            binary_files,
        });
    }
    for tool in &spec.tools {
        let artifact = artifacts
            .iter()
            .find(|artifact| artifact.name == tool.name)
            .ok_or_else(|| GeneratorError::MissingArtifact {
                tool: tool.name.clone(),
            })?;
        binary_files.insert(tool_artifact_path(&tool.name), artifact.bytes.clone());
    }
    for piece in spec.code_pieces() {
        let artifact = artifacts
            .iter()
            .find(|artifact| artifact.name == piece.name)
            .ok_or_else(|| GeneratorError::MissingArtifact {
                tool: piece.name.clone(),
            })?;
        binary_files.insert(tool_artifact_path(&piece.name), artifact.bytes.clone());
    }
    Ok(GeneratedCrate {
        files,
        binary_files,
    })
}

/// `spec` without `tool_packages`, with their tools as tools without code.
fn with_package_tools(spec: &AgentSpec, tools: &[grauwolf_tools::ResolvedTool]) -> AgentSpec {
    let mut derived = spec.clone();
    derived.tool_packages.clear();
    derived
        .tools
        .extend(tools.iter().map(|tool| grauwolf_spec::ToolSpec {
            name: tool.name.clone(),
            description: tool.description.clone(),
            args: tool.args.clone(),
            code: String::new(),
        }));
    derived
}

/// `build.rs` of a crate with tool sources: only the standard library, so the crate gets no
/// build dependencies. It stops the build with the command to run while a module is missing
/// (the library would otherwise fail with a bare `include_bytes!` error).
fn build_script(tools: &[grauwolf_tools::ToolSourceCrate]) -> String {
    let modules = tools
        .iter()
        .map(|tool| {
            format!(
                "    {:?},\n",
                tool_artifact_path(&tool.name).display().to_string()
            )
        })
        .collect::<String>();
    format!(
        "//! Generated – do not edit by hand (ADR-0095).\n\
         //!\n\
         //! The tools of this crate are shipped as sources under `tools/`; their WASM modules are\n\
         //! built by `grauwolf tools build .` before `cargo build`.\n\
         \n\
         const MODULES: &[&str] = &[\n{modules}];\n\
         \n\
         fn main() {{\n    \
             println!(\"cargo:rerun-if-changed=tools\");\n    \
             let missing: Vec<&str> = MODULES\n        \
                 .iter()\n        \
                 .copied()\n        \
                 .filter(|module| !std::path::Path::new(module).is_file())\n        \
                 .collect();\n    \
             if !missing.is_empty() {{\n        \
                 panic!(\n            \
                     \"tool modules missing: {{}} - run `grauwolf tools build .` in this directory first\",\n            \
                     missing.join(\", \")\n        \
                 );\n    \
             }}\n\
         }}\n"
    )
}

/// Location of a tool artifact relative to the crate.
fn tool_artifact_path(tool: &str) -> PathBuf {
    PathBuf::from("tools").join(format!("{tool}.wasm"))
}

/// `Cargo.toml` of the generated crate; `[workspace]` makes it independent of the surrounding
/// workspace.
fn manifest(
    spec: &AgentSpec,
    grauwolf: &GrauwolfDependency,
    options: &GenerateOptions,
    service: bool,
) -> String {
    let GrauwolfDependency::Path(root) = grauwolf;
    let dependency = |layer: &str, name: &str| {
        format!(
            "{name} = {{ path = {:?} }}\n",
            root.join(layer).join(name).display().to_string()
        )
    };
    let mut manifest = format!(
        "# Generated from the AgentSpec `{name}` – do not edit by hand.\n\n\
         [package]\nname = {name:?}\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [workspace]\n\n[dependencies]\n",
        name = spec.name
    );
    for name in [
        "grauwolf-agents",
        "grauwolf-graph",
        "grauwolf-llm",
        "grauwolf-prompt",
    ] {
        manifest.push_str(&dependency("domain", name));
    }
    if uses_plan(spec) {
        manifest.push_str(&dependency("domain", "grauwolf-planner"));
    }
    if !spec.types.is_empty() {
        manifest.push_str("schemars = \"1\"\n");
        manifest.push_str("serde = { version = \"1\", features = [\"derive\"] }\n");
    }
    // `run_observed` reports input and output as JSON in the events.
    manifest.push_str("serde_json = \"1\"\n");
    let has_code = !spec.code_pieces().is_empty();
    if !spec.tools.is_empty() || needs_external(spec) || has_code {
        manifest.push_str(&dependency("domain", "grauwolf-tools"));
    }
    if !spec.tools.is_empty() || has_code {
        manifest.push_str(&dependency("adapters", "grauwolf-sandbox"));
    }
    // Facade only with feature `app`, and only with the parts that `run_app` needs (ADR-0084).
    let mut facade = vec!["ollama", "openai", "anthropic", "gemini"];
    // Amazon Bedrock only when a model of the spec uses it (ADR-0105).
    if spec.providers().contains(&"bedrock") {
        facade.push("bedrock");
    }
    // PostgreSQL with pgvector only when knowledge or memory is stored there (ADR-0106).
    let postgres_knowledge = spec.knowledge.as_ref().is_some_and(|knowledge| {
        knowledge
            .store
            .as_ref()
            .is_some_and(|store| store.kind == grauwolf_spec::KnowledgeStoreKind::Postgres)
    });
    let postgres_memory = spec
        .long_term_memory
        .as_ref()
        .is_some_and(|memory| memory.vector_store.is_some());
    if postgres_knowledge || postgres_memory {
        facade.push("pgvector");
    }
    if needs_external(spec) {
        facade.extend(["mcp", "builtin"]);
        facade.extend(builtin_features(spec));
    }
    if !spec.agent_tools.is_empty() {
        facade.extend(["sandbox", "a2a"]);
    }
    // The System One client only when a decision node asks for it (ADR-0100).
    if spec.uses_system_one() {
        facade.push("systemone");
    }
    let facade_path = root.join("apps").join("grauwolf").display().to_string();
    let facade_dependency = if options.full_facade {
        format!("grauwolf = {{ path = {facade_path:?}, optional = true }}")
    } else {
        format!(
            "grauwolf = {{ path = {facade_path:?}, optional = true, default-features = false, features = {facade:?} }}"
        )
    };
    manifest.push_str(&format!(
        "{facade_dependency}\n\n[features]\napp = [\"dep:grauwolf\"]\n"
    ));
    if service {
        // The binary only exists with the feature; the name is the same for every spec, so that
        // images need not know it (ADR-0093).
        manifest.push_str(
            "service = [\"app\", \"grauwolf/service\", \"grauwolf/otel\", \"grauwolf/credentials\"]\n\n\
             [[bin]]\nname = \"agent\"\npath = \"src/main.rs\"\nrequired-features = [\"service\"]\n",
        );
    }
    manifest
}

/// `src/main.rs`: serves the agent through `grauwolf::service` (ADR-0093).
fn service_main(spec: &AgentSpec) -> String {
    let krate = spec.name.replace('-', "_");
    format!(
        "//! Service binary of the generated agent: `/run`, `/run/stream`, `/health`, A2A and MCP\n\
         //! (feature `service`; settings from the environment, see `grauwolf::service`).\n\n\
         use std::sync::Arc;\n\n\
         fn main() -> std::process::ExitCode {{\n    \
         grauwolf::service::main(\n        \
         {krate}::SPEC_JSON,\n        \
         Arc::new(|base_dir, options| {{\n            \
         Box::pin(async move {{\n                \
         {krate}::provision(&base_dir, options)\n                    \
         .await\n                    \
         .map_err(|error| error.to_string())\n            \
         }})\n        \
         }}),\n        \
         Arc::new(|agent, input, options| {{\n            \
         Box::pin(async move {{\n                \
         let input = serde_json::from_value(input).map_err(|error| error.to_string())?;\n                \
         let output = {krate}::run_provisioned(&agent, input, options)\n                    \
         .await\n                    \
         .map_err(|error| error.to_string())?;\n                \
         serde_json::to_value(output).map_err(|error| error.to_string())\n            \
         }})\n        \
         }}),\n    \
         )\n}}\n"
    )
}

/// Generates the source of `src/lib.rs` for `spec`.
pub fn generate_lib(spec: &AgentSpec) -> Result<String, GeneratorError> {
    generate_lib_limited(spec, &BTreeMap::new())
}

/// Like [`generate_lib`]; the tools named in `limits` (tools of packages that ask for sandbox
/// limits, ADR-0124) are loaded with `instantiate_limited`, so the sandbox applies the stricter
/// of their limits and its own.
pub fn generate_lib_limited(
    spec: &AgentSpec,
    limits: &BTreeMap<String, grauwolf_tools::ToolLimits>,
) -> Result<String, GeneratorError> {
    spec.validate().map_err(GeneratorError::Invalid)?;
    // Tools directory at runtime (ADR-0074) only in the interpreter.
    if spec.tools_dir.is_some() {
        return Err(GeneratorError::Unsupported {
            feature: "tools_dir".into(),
        });
    }
    // Self-generated tools (ADR-0075) need a build and the sandbox at runtime.
    if spec.self_tools.is_some() {
        return Err(GeneratorError::Unsupported {
            feature: "self_tools".into(),
        });
    }
    let external = spec.external_tools();
    let has_external = needs_external(spec);

    let mut scope = Scope::new();
    scope.import("std::sync", "Arc");
    scope.import("grauwolf_agents", "AgentContext");
    scope.import("grauwolf_graph", "GraphError");
    scope.import("grauwolf_graph", "TypedGraph");
    scope.import("grauwolf_llm", "PromptExecutor");
    scope.import("grauwolf_prompt", "LlModel");
    scope.import("grauwolf_prompt", "Prompt");

    types::push_types(&mut scope, spec)?;
    if !spec.code_pieces().is_empty() {
        scope.raw(CODE_MODULE_FN.to_owned());
    }
    scope.push_fn(graph_fn(spec)?);
    for function in subgraph_fns(spec)? {
        scope.push_fn(function);
    }

    scope
        .new_fn("model")
        .doc("Model from the spec.")
        .vis("pub")
        .ret("LlModel")
        .line(model_expr(&spec.model));

    let mut prompt = format!("Prompt::builder({:?})", spec.name);
    if let Some(system) = &spec.system_prompt {
        prompt.push_str(&format!(".system({system:?})"));
    }
    prompt.push_str(".build()");
    scope
        .new_fn("prompt")
        .doc("Initial prompt from the spec.")
        .vis("pub")
        .ret("Prompt")
        .line(prompt);

    let output = format!(
        "Result<{}, Box<dyn std::error::Error + Send + Sync>>",
        types::type_ref(&spec.output)
    );
    let memory = MemoryUse::of(spec);
    if memory.any() {
        scope.import("grauwolf_agents::memory", "FactMemory");
    }
    if has_external {
        // External tools come from the caller, e.g. from `ProvisionedAgent::tools_for` (ADR-0050, ADR-0114).
        scope.raw(format!(
            "/// Tools that must be provided from outside (MCP, knowledge, sub-agents, built-in tools).\n\
             pub const EXTERNAL_TOOLS: &[&str] = &{external:?};"
        ));
        scope.import("grauwolf_tools", "DynTool");
    }
    scope.import("grauwolf_agents::run_handles", "RunHandles");
    scope.import("grauwolf_agents::events", "Feature");
    let input = types::type_ref(&spec.input);
    // The entry points from outside in: each one calls the next with empty values.
    let mut entries: Vec<Entry> = vec![(
        "run",
        "Runs the agent with `input` in a fresh context.",
        Vec::new(),
    )];
    if has_external {
        entries.push((
            "run_with_tools",
            "Like [`run`]; `external` must contain all [`EXTERNAL_TOOLS`].",
            vec![("external", "Vec<Arc<dyn DynTool>>")],
        ));
    }
    if memory.any() {
        entries.push((
            "run_with_memory",
            "Like [`run`]; with `memory` the agent recalls facts and remembers new ones as the spec \
             asks (ADR-0069, ADR-0084).",
            vec![("memory", "Option<Arc<dyn FactMemory>>")],
        ));
    }
    let decider = spec.uses_system_one();
    if decider {
        scope.import("grauwolf_agents::decision", "Decider");
        entries.push((
            "run_with_decider",
            "Like [`run`]; `decider` answers the decision nodes that name `system_one` (ADR-0100).",
            vec![("decider", "Option<Arc<dyn Decider>>")],
        ));
    }
    entries.push((
        "run_observed",
        "Like [`run`]; `features` see all events of the run, in the same order as with the \
         interpreter (ADR-0084).",
        vec![("features", "Vec<Arc<dyn Feature>>")],
    ));
    let mut args: Vec<(&str, &str)> = Vec::new();
    for (index, (name, doc, own)) in entries.iter().enumerate() {
        args.extend(own.iter().copied());
        let function = scope
            .new_fn(*name)
            .doc(*doc)
            .vis("pub")
            .set_async(true)
            .arg("executor", "Arc<PromptExecutor>")
            .arg("input", &input);
        for (arg, ty) in &args {
            function.arg(*arg, *ty);
        }
        function.ret(&output);
        match entries.get(index + 1) {
            Some((next, _, added)) => {
                let mut call: Vec<String> = vec!["executor".into(), "input".into()];
                call.extend(args.iter().map(|(arg, _)| arg.to_string()));
                call.extend(added.iter().map(|(_, ty)| empty_value(ty)));
                function.line(format!("{next}({}).await", call.join(", ")));
            }
            None => {
                // The core takes the handles of the caller; a plain run has none (ADR-0119).
                let mut call: Vec<String> = vec!["executor".into(), "Some(input)".into()];
                call.extend(args.iter().map(|(arg, _)| arg.to_string()));
                call.push("RunHandles::default()".into());
                function.line(format!("run_handled({}).await", call.join(", ")));
            }
        }
    }
    let handled_input = format!("Option<{input}>");
    let handled = scope
        .new_fn("run_handled")
        .doc(
            "The core of all entry points: like [`run_observed`], with the handles of the caller – \
             prices, a budget of its own, an approver for tools that need approval, a frame sink, \
             a conversation, attachments and checkpoints; with `handles.resume` the run continues \
             from its checkpoint and `input` is `None` (ADR-0119).",
        )
        .vis("pub")
        .set_async(true)
        .arg("executor", "Arc<PromptExecutor>")
        .arg("input", &handled_input);
    for (arg, ty) in &args {
        handled.arg(*arg, *ty);
    }
    handled.arg("handles", "RunHandles").ret(&output);
    for line in run_body(spec, &memory, has_external, decider) {
        handled.line(line);
    }

    scope.raw(format!(
        "/// The spec of this crate as JSON (for `run_app`, ADR-0084).\npub const SPEC_JSON: &str = {:?};",
        spec.to_json()
    ));
    for function in run_app_fns(spec, &memory, &output) {
        scope.push_fn(function);
    }

    if !spec.tools.is_empty() || has_external {
        scope.import("grauwolf_tools", "ToolRegistry");
        if !spec.tools.is_empty() {
            scope.import("grauwolf_tools", "ToolArtifact");
            scope.import("grauwolf_tools", "ToolDescriptor");
            scope.import("grauwolf_tools", "ToolRuntime");
        }
        scope.push_fn(tools_fn(spec, has_external, limits));
    }

    // Import `AgentEvent` only when the code names it: an unused import is a warning in the
    // generated crate.
    let mut source = scope.to_formatted_string()?;
    if source.contains("AgentEvent") {
        scope.import("grauwolf_agents::events", "AgentEvent");
        source = scope.to_formatted_string()?;
    }
    // The inner doc must come before all items; codegen always puts imports first.
    Ok(format!(
        "//! Generated from the AgentSpec `{}` – do not edit by hand.\n\n{source}",
        spec.name,
    ))
}

/// `provision`, `run_provisioned`, `run_app` and `run_app_with` behind the feature `app`: they
/// set up everything through the facade like `grauwolf spec run` – one provisioning call
/// builds the agent once (providers, external tools, sub-agents, memory), a run per input
/// brings the user's handles and the events to the features (ADR-0084, ADR-0114).
fn run_app_fns(
    spec: &AgentSpec,
    memory: &MemoryUse,
    output: &str,
) -> [grauwolf_codegen::Function; 6] {
    let input = types::type_ref(&spec.input);
    let error = "Box<dyn std::error::Error + Send + Sync>";
    let provisioned = "grauwolf::provisioning::ProvisionedAgent";
    let options = "grauwolf::provisioning::ProvisionOptions";

    let mut provision = grauwolf_codegen::Function::new("provision");
    provision
        .doc(
            "Provisions the agent once: providers, external tools, sub-agents and long-term \
             memory with paths relative to `base_dir`, configuration from `options` (feature \
             `app`, ADR-0114).",
        )
        .attr("cfg(feature = \"app\")")
        .vis("pub")
        .set_async(true)
        .arg("base_dir", "&std::path::Path")
        .arg("options", options)
        .ret(format!("Result<{provisioned}, {error}>"))
        .line("use grauwolf::spec::AgentSpecExt as _;")
        .line("let spec = grauwolf::spec::AgentSpec::from_json(SPEC_JSON)?;");
    if !spec.agent_tools.is_empty() {
        provision.line(
            "let options = options.clone().with_sub_agents(Arc::new(\
             grauwolf::loader::AgentLoader::for_provisioning(\
             grauwolf::toolbuild::ToolBuilder::new(base_dir.join(\".grauwolf/tools\")), \
             &options)));",
        );
    }
    provision
        .line("Ok(grauwolf::provisioning::ProvisionedAgent::new(&spec, base_dir, options).await?)");

    let external = needs_external(spec);
    let output_ty = output;
    let mut run_with_options = grauwolf_codegen::Function::new("run_with_options");
    run_with_options
        .attr("cfg(feature = \"app\")")
        .set_async(true)
        .arg("provisioned", format!("&{provisioned}"))
        .arg("input", format!("Option<{input}>"))
        .arg("options", "grauwolf::app::RunOptions")
        .ret(output_ty);
    let mut call = vec![String::from("provisioned.executor()"), "input".into()];
    if external {
        run_with_options.line("let external = provisioned.tools_for(options.user.as_deref())?;");
        call.push("external".into());
    }
    if memory.any() {
        run_with_options.line("let memory = provisioned.run(options.user.as_deref())?.facts();");
        call.push("memory".into());
    }
    if spec.uses_system_one() {
        call.push("provisioned.decider()".into());
    }
    call.push("options.features.clone()".into());
    call.push("options.handles(provisioned.prices())".into());
    run_with_options.line(format!("run_handled({}).await", call.join(", ")));

    let mut run_provisioned = grauwolf_codegen::Function::new("run_provisioned");
    run_provisioned
        .doc(
            "Runs the provisioned agent for `options.user` (the area in long-term memory and the \
             key-value store); `options.features` see all events. The other options bring what \
             a caller of the interpreter has: a budget, a conversation, attachments, an approver, \
             a frame sink and checkpoints (feature `app`, ADR-0119).",
        )
        .attr("cfg(feature = \"app\")")
        .vis("pub")
        .set_async(true)
        .arg("provisioned", format!("&{provisioned}"))
        .arg("input", &input)
        .arg("options", "grauwolf::app::RunOptions")
        .ret(output)
        .line("run_with_options(provisioned, Some(input), options).await");

    let mut resume_provisioned = grauwolf_codegen::Function::new("resume_provisioned");
    resume_provisioned
        .doc(
            "Continues the run `run_id` from its last checkpoint in `store`; work before it is \
             not repeated, and the input is the one of the checkpoint (feature `app`, ADR-0119).",
        )
        .attr("cfg(feature = \"app\")")
        .vis("pub")
        .set_async(true)
        .arg("provisioned", format!("&{provisioned}"))
        .arg(
            "store",
            "Arc<dyn grauwolf_agents::checkpoints::CheckpointStore>",
        )
        .arg("run_id", "&str")
        .arg("options", "grauwolf::app::RunOptions")
        .ret(output)
        .line("run_with_options(provisioned, None, options.resuming(store, run_id)).await");

    let mut run_app = grauwolf_codegen::Function::new("run_app");
    run_app
        .doc(
            "Runs the agent like `grauwolf spec run`: providers from the process environment, \
             external tools, sub-agents and long-term memory (shared area) with paths relative \
             to `base_dir` (feature `app`).",
        )
        .attr("cfg(feature = \"app\")")
        .vis("pub")
        .set_async(true)
        .arg("base_dir", "&std::path::Path")
        .arg("input", &input)
        .ret(output)
        .line(
            "run_app_with(base_dir, input, grauwolf::provisioning::ProvisionOptions::new(\
             Arc::new(grauwolf::config::ProcessEnv)), grauwolf::app::RunOptions::default()).await",
        );

    let mut run_app_with = grauwolf_codegen::Function::new("run_app_with");
    run_app_with
        .doc(
            "Like [`run_app`], with the options of the agent (`provision`) and of the run \
             (`run_provisioned`): provisions the agent and runs it once (feature `app`). A \
             service provisions once and runs many times instead.",
        )
        .attr("cfg(feature = \"app\")")
        .vis("pub")
        .set_async(true)
        .arg("base_dir", "&std::path::Path")
        .arg("input", &input)
        .arg("provision_options", options)
        .arg("options", "grauwolf::app::RunOptions")
        .ret(output)
        .line("let provisioned = provision(base_dir, provision_options).await?;")
        .line("run_provisioned(&provisioned, input, options).await");
    [
        provision,
        run_with_options,
        run_provisioned,
        resume_provisioned,
        run_app,
        run_app_with,
    ]
}

/// Does the generated crate need tools from outside? Also for knowledge as context only: the
/// knowledge base comes with the provided tools (ADR-0113).
fn needs_external(spec: &AgentSpec) -> bool {
    !spec.external_tools().is_empty() || spec.knowledge.as_ref().is_some_and(|k| k.inject)
}

/// Automatic long-term memory of the spec (ADR-0069, ADR-0079).
struct MemoryUse {
    recall: bool,
    remember: bool,
    compressed: bool,
}

impl MemoryUse {
    fn of(spec: &AgentSpec) -> Self {
        let memory = spec.long_term_memory.as_ref();
        Self {
            recall: memory.is_some_and(|memory| memory.auto_recall),
            remember: memory.is_some_and(|memory| memory.auto_remember),
            compressed: memory.is_some_and(|memory| memory.remember_compressed),
        }
    }

    fn any(&self) -> bool {
        self.recall || self.remember || self.compressed
    }
}

/// An entry point of the generated crate: name, docs and the arguments it adds.
type Entry<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>);

/// Expression for a model of the spec including its fallback models (ADR-0088).
/// The facade features for the groups of built-in tools the spec uses (ADR-0108): without them
/// the generated crate would refuse the tools at start.
fn builtin_features(spec: &AgentSpec) -> Vec<&'static str> {
    let mut features: Vec<&'static str> = spec
        .builtin_tools
        .iter()
        .flat_map(grauwolf_spec::BuiltinToolsSpec::groups)
        .filter_map(group_feature)
        .collect();
    features.sort_by_key(|feature| FEATURE_ORDER.iter().position(|known| known == feature));
    features.dedup();
    features
}

/// The facade features in the order they are written into the generated `Cargo.toml`.
const FEATURE_ORDER: &[&str] = &["git", "fetch", "command", "atlassian"];

/// The facade feature that a group of built-in tools needs; `None` for the groups that every
/// build has. The table of groups lives in the domain, features do not (ADR-0016, ADR-0117).
fn group_feature(group: grauwolf_spec::BuiltinGroup) -> Option<&'static str> {
    use grauwolf_spec::BuiltinGroup as G;
    match group {
        G::Git => Some("git"),
        G::Fetch => Some("fetch"),
        G::Command => Some("command"),
        G::Jira | G::Confluence => Some("atlassian"),
        G::User | G::Think | G::Todo | G::Kv | G::File => None,
    }
}

fn model_expr(model: &grauwolf_spec::ModelSpec) -> String {
    let mut expr = sampling_expr(&model.provider, &model.id, model.temperature, model.think);
    if !model.fallbacks.is_empty() {
        let fallbacks: Vec<String> = model
            .fallbacks
            .iter()
            .map(|fallback| {
                sampling_expr(
                    &fallback.provider,
                    &fallback.id,
                    fallback.temperature,
                    fallback.think,
                )
            })
            .collect();
        expr.push_str(&format!(".with_fallbacks(vec![{}])", fallbacks.join(", ")));
    }
    expr
}

/// `LlModel::new(..)` with `with_temperature`/`with_think` where set (ADR-0097).
fn sampling_expr(
    provider: &str,
    id: &str,
    temperature: Option<f64>,
    think: Option<bool>,
) -> String {
    let mut expr = format!("LlModel::new({provider:?}, {id:?})");
    if let Some(temperature) = temperature {
        expr.push_str(&format!(".with_temperature({temperature:?})"));
    }
    if let Some(think) = think {
        expr.push_str(&format!(".with_think({think})"));
    }
    expr
}

/// Empty value for an argument that an outer entry point does not offer.
fn empty_value(ty: &str) -> String {
    if ty.starts_with("Option") {
        "None".into()
    } else {
        "Vec::new()".into()
    }
}

/// Body of `run_observed`: context with tools, features, budget and long-term memory, then one
/// call of the run lifecycle (ADR-0116), which owns the order of events, recall, knowledge and
/// remember – the same sequence as in the interpreter.
fn run_body(spec: &AgentSpec, memory: &MemoryUse, external: bool, decider: bool) -> Vec<String> {
    let tools = if external {
        ".with_tools(tools(external)?)"
    } else if spec.tools.is_empty() {
        ""
    } else {
        ".with_tools(tools()?)"
    };
    let limits = budget_expr(spec);
    let approval = format!("&{:?}", spec.approval_tools());
    let mut lines = vec!["let state = handles.resume_state().await?;".to_owned()];
    if let Some(knowledge) = spec.knowledge.as_ref().filter(|knowledge| knowledge.inject) {
        // Before `tools(external)` consumes the tools (ADR-0113); looked up by the name of the
        // search tool with the interpreter's error text (ADR-0116).
        lines.push(format!(
            "let knowledge = grauwolf_tools::knowledge_context_of(&external, {name:?}).ok_or(\"tool '{name}' was not provided (MCP server, knowledge, sub-agent or built-in tool)\")?;",
            name = knowledge.tool
        ));
    }
    lines.extend(if spec.skills.is_empty() {
        vec![format!(
            "let mut ctx = handles.context(executor, model(), prompt(), state.as_ref(), {limits}, {approval})?{tools};"
        )]
    } else {
        // The tools extend the system prompt with the list of skills, as in the interpreter
        // (ADR-0103); skills come with external tools.
        vec![
            "let registry = tools(external)?;".to_owned(),
            "let prompt = match registry.prompt_addition() { Some(addition) => prompt().extend_system(&addition), None => prompt() };".to_owned(),
            format!("let mut ctx = handles.context(executor, model(), prompt, state.as_ref(), {limits}, {approval})?.with_tools(registry);"),
        ]
    });
    lines.push("for feature in features { ctx = ctx.install(feature); }".into());
    if decider {
        lines.push(
            "if let Some(decider) = &decider { ctx = ctx.with_decider(Arc::clone(decider)); }"
                .into(),
        );
    }
    lines.push("let run = handles.start(input.as_ref(), state.as_ref())?;".into());
    if memory.any() {
        lines.push(format!(
            "let run = match &memory {{ Some(memory) => run.with_memory(Arc::clone(memory), {}, {}), None => run }};",
            memory.recall, memory.remember
        ));
    }
    if memory.compressed {
        // The lifecycle gives the memory to the history compression, as in the interpreter.
        lines.push("let run = run.remember_compressed();".into());
    }
    if spec
        .knowledge
        .as_ref()
        .is_some_and(|knowledge| knowledge.inject)
    {
        lines.push("let run = run.with_knowledge(knowledge);".into());
    }
    lines.push("let graph = graph()?;".into());
    lines.push("Ok(RunHandles::run_graph(run, &mut ctx, graph, input, state).await?)".into());
    lines
}

/// `pub fn tools()`: loads the embedded tool artifacts into the sandbox (ADR-0014); with
/// `external`, the checked external tools are added (ADR-0050).
fn tools_fn(
    spec: &AgentSpec,
    external: bool,
    limits: &BTreeMap<String, grauwolf_tools::ToolLimits>,
) -> grauwolf_codegen::Function {
    let mut function = grauwolf_codegen::Function::new("tools");
    function
        .doc(if external {
            "Tools of the spec (code in the sandbox) and the external tools from `external`."
        } else {
            "Tools of the spec; their code runs in the sandbox."
        })
        .vis("pub")
        .ret("Result<ToolRegistry, Box<dyn std::error::Error + Send + Sync>>");
    if external {
        function
            .arg("external", "Vec<Arc<dyn DynTool>>")
            .line("if let Some(missing) = EXTERNAL_TOOLS.iter().find(|name| !external.iter().any(|tool| tool.name() == **name)) {")
            .line("return Err(format!(\"tool '{missing}' was not provided (MCP server, knowledge, sub-agent or built-in tool)\").into());")
            .line("}");
    }
    if !spec.tools.is_empty() {
        function.line("let runtime = grauwolf_sandbox::SandboxRuntime::new()?;");
    }
    function.line("let registry = ToolRegistry::builder()");
    for tool in &spec.tools {
        let (method, limits) = match limits.get(&tool.name) {
            Some(limits) => (
                "instantiate_limited",
                format!(
                    ", &grauwolf_tools::ToolLimits {{ memory_mib: {:?}, fuel: {:?}, time_ms: {:?} }}",
                    limits.memory_mib, limits.fuel, limits.time_ms
                ),
            ),
            None => ("instantiate", String::new()),
        };
        function.line(format!(
            ".shared(runtime.{method}(\
             &ToolArtifact {{ name: {name:?}.into(), bytes: include_bytes!(\"../{path}\").to_vec(), hash: String::new() }}, \
             ToolDescriptor {{ name: {name:?}.into(), description: {description:?}.into(), \
             parameters: serde_json::from_str({parameters:?})? }}{limits})?)",
            name = tool.name,
            path = tool_artifact_path(&tool.name).display(),
            description = tool.description,
            parameters = tool.args.to_string(),
        ));
    }
    if external {
        function
            .line(";")
            .line("let registry = external.into_iter().filter(|tool| EXTERNAL_TOOLS.contains(&tool.name())).fold(registry, |registry, tool| registry.shared(tool));")
            .line("Ok(registry.build())");
    } else {
        function.line(".build();").line("Ok(registry)");
    }
    function
}

/// Loads an embedded code module of a graph into the sandbox, once per process (ADR-0102): the
/// graphs are built for every run, the modules are compiled only the first time.
const CODE_MODULE_FN: &str = r#"fn code_module(name: &'static str, bytes: &'static [u8]) -> grauwolf_agents::code::CodeModule {
    static MODULES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<&'static str, grauwolf_agents::code::CodeModule>>> = std::sync::OnceLock::new();
    let mut modules = MODULES.get_or_init(Default::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    modules
        .entry(name)
        .or_insert_with(|| {
            let runtime = grauwolf_sandbox::SandboxRuntime::new().expect("the sandbox starts");
            let artifact = grauwolf_tools::ToolArtifact { name: name.into(), bytes: bytes.to_vec(), hash: String::new() };
            grauwolf_agents::code::CodeModule::load(&runtime, &artifact).expect("the embedded code module is valid")
        })
        .clone()
}"#;

/// `pub fn graph()`: builds the strategy graph with the typed builder.
fn graph_fn(spec: &AgentSpec) -> Result<grauwolf_codegen::Function, GeneratorError> {
    graph_function(
        "graph",
        "Strategy graph of the agent.",
        spec,
        &spec.graph,
        (&spec.input, &spec.output),
    )
}

/// `pub fn subgraph_<name>()` for each subgraph of the spec (ADR-0034).
fn subgraph_fns(spec: &AgentSpec) -> Result<Vec<grauwolf_codegen::Function>, GeneratorError> {
    spec.subgraphs
        .iter()
        .map(|(name, subgraph)| {
            graph_function(
                &subgraph_fn_name(name),
                &format!("Subgraph `{name}`."),
                spec,
                &subgraph.graph,
                (&subgraph.input, &subgraph.output),
            )
        })
        .collect()
}

/// Expression for the limits of the spec (ADR-0078), the base of the caller's budget.
fn budget_expr(spec: &AgentSpec) -> String {
    let limits = spec.limits.as_ref();
    let max_tokens = limits.and_then(|limits| limits.max_tokens);
    let max_cost = limits.and_then(|limits| limits.max_cost);
    let max_tool_calls = limits.and_then(|limits| limits.max_tool_calls);
    let max_prompt_tokens = limits.and_then(|limits| limits.max_prompt_tokens);
    if max_tokens.is_none()
        && max_cost.is_none()
        && max_tool_calls.is_none()
        && max_prompt_tokens.is_none()
    {
        return "grauwolf_agents::Budget::default()".to_owned();
    }
    format!(
        "grauwolf_agents::Budget {{ max_tokens: {max_tokens:?}, max_cost: {max_cost:?}, max_tool_calls: {max_tool_calls:?}, max_prompt_tokens: {max_prompt_tokens:?} }}"
    )
}

/// Function `name` that builds `graph` with the typed signature `(input, output)`. Subgraphs
/// are built before the builder, because the closure cannot return errors.
fn graph_function(
    name: &str,
    doc: &str,
    spec: &AgentSpec,
    graph: &GraphSpec,
    (input, output): (&str, &str),
) -> Result<grauwolf_codegen::Function, GeneratorError> {
    let mut function = grauwolf_codegen::Function::new(name);
    function.doc(doc).vis("pub").ret(format!(
        "Result<TypedGraph<AgentContext, {}, {}>, GraphError>",
        types::type_ref(input),
        types::type_ref(output)
    ));
    // The code of this graph (ADR-0102): one sandboxed module per piece, found by the pointer of
    // its code in the spec.
    let path = spec.graph_pointer(graph).unwrap_or_default();
    let pieces = spec.code_pieces();
    let piece_at = |pointer: String| pieces.iter().find(|piece| piece.pointer == pointer);
    for piece in pieces
        .iter()
        .filter(|piece| piece.pointer.starts_with(&format!("{path}/")))
    {
        function.line(format!(
            "let {} = code_module({:?}, include_bytes!(\"../{}\"));",
            module_var(piece),
            piece.name,
            tool_artifact_path(&piece.name).display()
        ));
    }
    let mut body = Block::new("TypedGraph::build(|g|");
    // Step limit per graph run, as in the interpreter (ADR-0078).
    if let Some(steps) = spec.limits.as_ref().and_then(|limits| limits.max_steps) {
        body.line(format!("g.max_steps({steps});"));
    }
    for node in &graph.nodes {
        let params = node.params.as_ref().unwrap_or(&Value::Null);
        if node.kind == GOAP_KIND {
            // Same construction as in the interpreter: `fact_planner` from the parameters
            // (ADR-0070).
            let steps = format!("steps_{}", node.id);
            function.line(format!(
                "let mut {steps} = std::collections::HashMap::new();"
            ));
            let mut seen = std::collections::HashSet::new();
            for action in params["actions"].as_array().into_iter().flatten() {
                // An action with an `agent` runs the sub-agent from the loader as a step.
                let Some(called) = action["subgraph"].as_str() else {
                    continue;
                };
                if !seen.insert(called) {
                    continue;
                }
                let graph = format!("{}()?.into_graph()", subgraph_fn_name(called));
                let graph = match spec.subgraphs.get(called).and_then(scope_expr) {
                    Some(scope) => format!(
                        "grauwolf_graph::Graph::builder().subgraph(\"step\", {graph}, Some(Box::new({scope}))).edge(grauwolf_graph::START, \"step\").edge(\"step\", grauwolf_graph::FINISH).build()?"
                    ),
                    None => graph,
                };
                function.line(format!(
                    "{steps}.insert({called:?}.to_owned(), Arc::new({graph}));"
                ));
            }
            let max_iterations = params["max_iterations"]
                .as_u64()
                .unwrap_or(DEFAULT_PLAN_ITERATIONS);
            let built = format!("planner_{}", node.id);
            function.line(format!(
                "let {built} = Arc::new(grauwolf_planner::goap::fact_planner(&serde_json::from_str::<serde_json::Value>({:?}).expect(\"spec parameters are JSON\"), &{steps}).expect(\"spec parameters are checked\"));",
                params.to_string()
            ));
            body.line(format!(
                "let {} = g.node({:?}, grauwolf_planner::planner_node({built}, {max_iterations}, {:?}));",
                node_var(&node.id),
                node.id,
                node.id
            ));
            continue;
        }
        if node.kind == PLAN_KIND {
            let called = params
                .get("subgraph")
                .and_then(Value::as_str)
                .ok_or_else(|| missing(&node.kind))?;
            let mut planner = format!(
                "grauwolf_planner::LlmPlanner::new(Arc::new({}()?.into_graph()))",
                subgraph_fn_name(called)
            );
            if let Some(scope) = spec.subgraphs.get(called).and_then(scope_expr) {
                planner.push_str(&format!(".with_step_scope(Box::new({scope}))"));
            }
            if params["critic"].as_bool() == Some(true) {
                planner.push_str(".with_critic()");
            }
            let max_iterations = params["max_iterations"]
                .as_u64()
                .unwrap_or(DEFAULT_PLAN_ITERATIONS);
            let built = format!("planner_{}", node.id);
            function.line(format!("let {built} = Arc::new({planner});"));
            body.line(format!(
                "let {} = g.node({:?}, grauwolf_planner::plan_node({built}, {max_iterations}, {:?}));",
                node_var(&node.id),
                node.id,
                node.id
            ));
            continue;
        }
        if node.kind == PARALLEL_KIND {
            let mut branches = Vec::new();
            let mut signature = None;
            for name in params["subgraphs"].as_array().into_iter().flatten() {
                let name = name.as_str().ok_or_else(|| missing(&node.kind))?;
                let subgraph = spec
                    .subgraphs
                    .get(name)
                    .ok_or_else(|| missing(&node.kind))?;
                signature = Some((
                    types::type_ref(&subgraph.input),
                    types::type_ref(&subgraph.output),
                ));
                let mut branch = format!(
                    "grauwolf_agents::parallel::Branch::new({name:?}, {}()?.into_graph())",
                    subgraph_fn_name(name)
                );
                if let Some(scope) = scope_expr(subgraph) {
                    branch.push_str(&format!(".with_scope(Box::new({scope}))"));
                }
                branches.push(branch);
            }
            let (input, output) = signature.ok_or_else(|| missing(&node.kind))?;
            let merge = match params["merge"].as_str() {
                Some("concat") => "grauwolf_agents::parallel::Merge::Concat".to_owned(),
                Some("index") => format!(
                    "grauwolf_agents::parallel::Merge::Index({})",
                    params["index"]
                        .as_u64()
                        .ok_or_else(|| missing(&node.kind))?
                ),
                Some("llm_select") => format!(
                    "grauwolf_agents::parallel::Merge::LlmSelect {{ criteria: {}.into() }}",
                    params["criteria"].as_str().map_or(
                        "grauwolf_agents::nodes::DEFAULT_BEST_OF_CRITERIA".to_owned(),
                        |criteria| format!("{criteria:?}")
                    )
                ),
                _ => return Err(missing(&node.kind)),
            };
            let built = format!("branches_{}", node.id);
            function.line(format!("let {built} = vec![{}];", branches.join(", ")));
            body.line(format!(
                "let {} = g.node({:?}, grauwolf_agents::parallel::parallel::<{input}, {output}>({built}, {merge}, {:?}));",
                node_var(&node.id),
                node.id,
                node.id
            ));
            continue;
        }
        if node.kind == FOR_EACH_KIND {
            // Same construction as in the interpreter; the runtime is `for_each::for_each`
            // (ADR-0101).
            let parsed = ForEachParams::parse(Some(params)).map_err(|_| missing(&node.kind))?;
            let body_expr = match &parsed.body {
                ForEachBody::Agent(name) => {
                    format!("grauwolf_agents::for_each::Body::Agent({name:?}.to_owned())")
                }
                ForEachBody::Subgraph(name) => {
                    let scope = match spec.subgraphs.get(name).and_then(scope_expr) {
                        Some(scope) => format!("Some(Box::new({scope}))"),
                        None => "None".to_owned(),
                    };
                    format!(
                        "grauwolf_agents::for_each::Body::Graph {{ name: {name:?}.to_owned(), graph: {}()?.into_graph(), scope: {scope} }}",
                        subgraph_fn_name(name)
                    )
                }
            };
            let mut config = format!(
                "grauwolf_agents::for_each::ForEach::new({body_expr}).with_max_parallel({})",
                parsed.max_parallel
            );
            if parsed.on_error == ForEachOnError::Skip {
                config.push_str(".with_on_error(grauwolf_agents::for_each::OnError::Skip)");
            }
            if parsed.output == "string" {
                config.push_str(".with_text_output()");
            }
            let built = format!("each_{}", node.id);
            function.line(format!("let {built} = {config};"));
            body.line(format!(
                "let {} = g.node({:?}, grauwolf_agents::for_each::for_each::<{}, {}>({built}, {:?}));",
                node_var(&node.id),
                node.id,
                types::type_ref(&parsed.input),
                types::type_ref(&parsed.output),
                node.id
            ));
            continue;
        }
        if node.kind == SUBGRAPH_KIND {
            let called = params
                .get("subgraph")
                .and_then(Value::as_str)
                .ok_or_else(|| missing(&node.kind))?;
            let scope = match spec.subgraphs.get(called).and_then(scope_expr) {
                Some(scope) => format!("Some(Box::new({scope}))"),
                None => "None".to_owned(),
            };
            let built = format!("sub_{}", node.id);
            function.line(format!("let {built} = {}()?;", subgraph_fn_name(called)));
            body.line(format!(
                "let {} = g.subgraph({:?}, {built}, {scope});",
                node_var(&node.id),
                node.id
            ));
            continue;
        }
        if node.kind == CODE_KIND {
            let index = graph.nodes.iter().position(|other| other.id == node.id);
            let piece = index
                .and_then(|index| piece_at(format!("{path}/nodes/{index}/params/code")))
                .ok_or_else(|| missing(&node.kind))?;
            body.line(format!(
                "let {} = g.node({:?}, grauwolf_agents::code::code_node_typed::<{}, {}>({}.clone()));",
                node_var(&node.id),
                node.id,
                types::type_ref(&piece.input),
                types::type_ref(&piece.output),
                module_var(piece)
            ));
            continue;
        }
        body.line(format!(
            "let {} = g.node({:?}, {});",
            node_var(&node.id),
            node.id,
            templates::node_for(&node.id, &node.kind, params).ok_or_else(|| missing(&node.kind))?
        ));
    }
    for (index, edge) in graph.edges.iter().enumerate() {
        let condition = edge
            .when
            .as_ref()
            .filter(|when| when.kind == ON_CODE_KIND)
            .map(|_| piece_at(format!("{path}/edges/{index}/when/params/code")))
            .map(|piece| piece.ok_or_else(|| missing(ON_CODE_KIND)))
            .transpose()?;
        let transform = edge
            .transform
            .as_ref()
            .map(|_| piece_at(format!("{path}/edges/{index}/transform/code")))
            .map(|piece| piece.ok_or_else(|| missing("transform")))
            .transpose()?;
        body.line(edge_line(edge, condition, transform)?);
    }
    body.after(")");
    function.push_block(body);
    Ok(function)
}

/// Name of the function that builds the subgraph `name`.
/// Rust expression for the scope of a subgraph: only tools → `ToolSubset`, with its own model →
/// `SubgraphScope` (ADR-0034, ADR-0043); neither gives none.
fn scope_expr(subgraph: &SubgraphSpec) -> Option<String> {
    match (&subgraph.tools, &subgraph.model) {
        (None, None) => None,
        (Some(tools), None) => Some(format!(
            "grauwolf_agents::scopes::ToolSubset::new({tools:?})"
        )),
        (tools, Some(model)) => {
            let mut scope = "grauwolf_agents::scopes::SubgraphScope::new()".to_owned();
            if let Some(tools) = tools {
                scope.push_str(&format!(".with_tools({tools:?})"));
            }
            scope.push_str(&format!(
                ".with_model({})",
                model_expr(model).replace("LlModel::new", "grauwolf_prompt::LlModel::new")
            ));
            Some(scope)
        }
    }
}

/// `true` if a graph of the spec contains a node of the kind `kind`.
fn uses_kind(spec: &AgentSpec, kind: &str) -> bool {
    std::iter::once(&spec.graph)
        .chain(spec.subgraphs.values().map(|subgraph| &subgraph.graph))
        .any(|graph| graph.nodes.iter().any(|node| node.kind == kind))
}

/// `true` if a graph of the spec contains a planner node (`plan`, `goap`).
fn uses_plan(spec: &AgentSpec) -> bool {
    uses_kind(spec, PLAN_KIND) || uses_kind(spec, GOAP_KIND)
}

fn subgraph_fn_name(name: &str) -> String {
    format!("subgraph_{name}")
}

/// Line of an edge. Edges with code (ADR-0102) pass the condition and the transformation as
/// built modules to `edge_with`: the types are checked by the validation and when the target
/// reads its input.
fn edge_line(
    edge: &EdgeSpec,
    condition: Option<&CodePiece>,
    transform: Option<&CodePiece>,
) -> Result<String, GeneratorError> {
    let (from, to) = (handle(&edge.from), handle(&edge.to));
    if condition.is_some() || transform.is_some() {
        let condition = match (condition, &edge.when) {
            (Some(piece), _) => format!(
                "Some(grauwolf_agents::code::code_condition({}.clone()))",
                module_var(piece)
            ),
            (None, Some(when)) => format!(
                "Some(grauwolf_agents::implementations::condition({:?}, &serde_json::from_str::<serde_json::Value>({:?}).expect(\"spec parameters are JSON\")).expect(\"the condition is in the catalog\"))",
                when.kind,
                when.params.as_ref().unwrap_or(&Value::Null).to_string()
            ),
            (None, None) => "None".to_owned(),
        };
        let transform = transform.map_or_else(
            || "None".to_owned(),
            |piece| {
                format!(
                    "Some(grauwolf_agents::code::code_transform({}.clone()))",
                    module_var(piece)
                )
            },
        );
        return Ok(format!(
            "g.edge_with(&{from}, &{to}, {condition}, {transform});"
        ));
    }
    Ok(match &edge.when {
        None => format!("g.edge(&{from}, &{to});"),
        Some(when) => format!(
            "g.edge_when(&{from}, &{to}, {});",
            templates::condition(&when.kind, when.params.as_ref().unwrap_or(&Value::Null))
                .ok_or_else(|| missing(&when.kind))?
        ),
    })
}

/// Variable of the loaded module of a code piece.
fn module_var(piece: &CodePiece) -> String {
    format!("module_{}", piece.name)
}

/// Expression for the handle of the node `id` in the builder.
fn handle(id: &str) -> String {
    match id {
        START => "g.start()".to_owned(),
        FINISH => "g.finish()".to_owned(),
        _ => node_var(id),
    }
}

/// Variable name for the node `id`; the prefix prevents collisions with Rust keywords.
fn node_var(id: &str) -> String {
    format!("node_{id}")
}

fn missing(kind: &str) -> GeneratorError {
    GeneratorError::MissingTemplate {
        kind: kind.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use grauwolf_spec::{BUILTIN_GROUPS, BuiltinGroup};

    use super::{FEATURE_ORDER, group_feature};

    #[test]
    fn every_group_is_mapped_to_a_known_feature_or_to_none() {
        let with_feature: Vec<BuiltinGroup> = BUILTIN_GROUPS
            .iter()
            .map(|info| info.group)
            .filter(|group| group_feature(*group).is_some())
            .collect();
        assert_eq!(
            with_feature,
            [
                BuiltinGroup::Fetch,
                BuiltinGroup::Command,
                BuiltinGroup::Jira,
                BuiltinGroup::Confluence,
                BuiltinGroup::Git
            ]
        );
        for group in with_feature {
            assert!(FEATURE_ORDER.contains(&group_feature(group).unwrap()));
        }
    }
}
