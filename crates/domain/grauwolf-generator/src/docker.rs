//! `Dockerfile` of a generated service (ADR-0096): builds offline in the builder image and
//! runs in a small runtime image.

use grauwolf_spec::AgentSpec;

/// Where the builder image keeps the pre-built dependencies and the workspace lock file.
const BUILDER_ROOT: &str = "/opt/grauwolf";

/// Settings of the generated `Dockerfile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerfileOptions {
    /// Builder image with tag, e.g. `ghcr.io/agentsw0rk/grauwolf-builder:0.1.0`.
    pub builder_image: String,
    /// Version of the grauwolf that generates the crate. When set, the first build step fails
    /// unless the builder image's `GRAUWOLF_VERSION` is exactly this; `None` omits the guard.
    pub expected_builder_version: Option<String>,
}

/// The builder image of a release: `ghcr.io/agentsw0rk/grauwolf-builder:<version>`.
pub fn default_builder_image(version: &str) -> String {
    format!("ghcr.io/agentsw0rk/grauwolf-builder:{version}")
}

/// Directory (relative to the crate) whose content becomes `/app` in the image: the files the
/// service reads at run time (knowledge sources, the root of the file tools, sub-agent specs).
pub const APP_DIR: &str = "app";

/// What a companion path of a spec is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanionKind {
    /// A file or directory of `knowledge.sources`.
    KnowledgeSource,
    /// The `knowledge.index` file.
    KnowledgeIndex,
    /// The `long_term_memory.store` file.
    MemoryStore,
    /// The `builtin_tools.root` of the file tools.
    FilesRoot,
    /// A local skill directory (`skills[].dir`).
    SkillDir,
}

/// One file or directory the service reads at run time, with the JSON pointer of the spec field
/// that names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Companion {
    /// JSON pointer of the field, e.g. `/knowledge/sources/0` or `/skills/1/dir`.
    pub pointer: String,
    /// The path as written in the spec (relative to the spec's directory).
    pub path: String,
    /// What it is.
    pub kind: CompanionKind,
}

/// Paths (relative to the spec's directory) of the files and directories the service reads at
/// run time: knowledge sources and index, the long-term memory store, the root of the file tools,
/// local skill directories.
/// Sub-agent specs (`agent_tools`) are not included; they have their own companions.
pub fn companion_paths(spec: &AgentSpec) -> Vec<String> {
    companions(spec)
        .into_iter()
        .map(|companion| companion.path)
        .collect()
}

/// [`companion_paths`] with the JSON pointer and the kind of each path.
pub fn companions(spec: &AgentSpec) -> Vec<Companion> {
    let mut companions = Vec::new();
    let mut add = |pointer: String, path: &String, kind| {
        companions.push(Companion {
            pointer,
            path: path.clone(),
            kind,
        })
    };
    if let Some(knowledge) = &spec.knowledge {
        for (index, source) in knowledge.sources.iter().enumerate() {
            add(
                format!("/knowledge/sources/{index}"),
                source,
                CompanionKind::KnowledgeSource,
            );
        }
        if let Some(index) = &knowledge.index {
            add(
                "/knowledge/index".into(),
                index,
                CompanionKind::KnowledgeIndex,
            );
        }
    }
    if let Some(store) = spec
        .long_term_memory
        .as_ref()
        .and_then(|memory| memory.store.as_ref())
    {
        add(
            "/long_term_memory/store".into(),
            store,
            CompanionKind::MemoryStore,
        );
    }
    if let Some(root) = spec
        .builtin_tools
        .as_ref()
        .and_then(|builtin| builtin.root.as_ref())
    {
        add("/builtin_tools/root".into(), root, CompanionKind::FilesRoot);
    }
    // Skill directories (ADR-0103); git sources are copied from the cache by the caller.
    for (index, source) in spec.skills.iter().enumerate() {
        if let Some(dir) = &source.dir {
            add(format!("/skills/{index}/dir"), dir, CompanionKind::SkillDir);
        }
    }
    companions
}

/// `Dockerfile` for `spec`.
pub fn dockerfile(spec: &AgentSpec, options: &DockerfileOptions, tool_sources: bool) -> String {
    // Tool sources first, built in a layer of their own: the layer is reused while the tools do
    // not change, whatever else changes in the crate (ADR-0095).
    let tools = if tool_sources {
        "# The tools are compiled to WASM first, in a layer of their own (ADR-0095).\n\
         COPY tools/ tools/\n\
         RUN grauwolf tools build . --cache /tmp/tool-cache\n"
    } else {
        ""
    };
    // The first build step: a crate must be built by the builder of the grauwolf that wrote it
    // (the vendored versions and the pre-built dependencies belong to that release).
    let guard = match &options.expected_builder_version {
        Some(version) => format!(
            "# The builder must be the version of the grauwolf that generated this crate.\n\
             RUN test \"$GRAUWOLF_VERSION\" = \"{version}\" || {{ \\\n    \
             echo \"crate generated by grauwolf {version}, builder is $GRAUWOLF_VERSION - \
             regenerate with grauwolf $GRAUWOLF_VERSION or pass --builder-image with \
             --allow-builder-mismatch\" >&2; exit 1; }}\n"
        ),
        None => String::new(),
    };
    let runtime = "FROM gcr.io/distroless/cc-debian12:nonroot\n\
                   COPY --from=build /usr/local/bin/agent /usr/local/bin/agent\n\
                   COPY --chown=65532:65532 app/ /app/\n";
    format!(
        "# Generated from the AgentSpec `{name}` – do not edit by hand (ADR-0096).\n\
         #\n\
         # The builder image has all dependencies vendored and compiled, so this build needs no\n\
         # network (`docker build --network none .`) and compiles only the agent crate.\n\
         FROM {builder} AS build\n\
         {guard}\
         WORKDIR /src\n\
         {tools}\
         COPY . .\n\
         # The lock file of the builder: the versions of the pre-built dependencies.\n\
         RUN cp {root}/Cargo.lock Cargo.lock \\\n    \
         && CARGO_TARGET_DIR={root}/target cargo build --release --offline --features service \\\n    \
         && cp {root}/target/release/agent /usr/local/bin/agent\n\n\
         {runtime}\
         WORKDIR /app\n\
         ENV GRAUWOLF_DATA_DIR=/app SERVER_PORT=8080\n\
         EXPOSE 8080\n\
         ENTRYPOINT [\"/usr/local/bin/agent\"]\n",
        name = spec.name,
        builder = options.builder_image,
        root = BUILDER_ROOT,
    )
}

/// `.dockerignore`: the build context is the crate directory; build output stays out.
pub fn dockerignore() -> &'static str {
    "target/\nDockerfile\n.dockerignore\n"
}
