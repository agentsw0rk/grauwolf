//! A generated project for an image: the files the service reads at run time in `app/`
//! (ADR-0096, ADR-0115) – shared by `grauwolf spec generate --dockerfile`, `grauwolf harness
//! generate` and grauwolf studio (ADR-0128).

//! The functions that read the agent tree need the features of `agent_tree` (`mcp`, `builtin`,
//! `sandbox`, `a2a`); the file names and `write_crate` are there in every build, because the
//! server and the crates a generated service builds with fewer features use them.

use std::path::Path;
#[cfg(all(
    feature = "mcp",
    feature = "builtin",
    feature = "sandbox",
    feature = "a2a"
))]
use std::path::PathBuf;

use grauwolf_generator::GeneratedCrate;
#[cfg(all(
    feature = "mcp",
    feature = "builtin",
    feature = "sandbox",
    feature = "a2a"
))]
use grauwolf_spec::Harness;
#[cfg(all(
    feature = "mcp",
    feature = "builtin",
    feature = "sandbox",
    feature = "a2a"
))]
use grauwolf_toolbuild::ToolBuilder;

#[cfg(all(
    feature = "mcp",
    feature = "builtin",
    feature = "sandbox",
    feature = "a2a"
))]
use crate::agent_tree::{AgentTree, ResolveOptions};

/// Where the builder image keeps the grauwolf crates (ADR-0096).
pub const BUILDER_CRATES: &str = "/opt/grauwolf/crates";

/// The note in every harness project: where it comes from and that it is output only (ADR-0128).
pub const NOTE_FILE: &str = "GENERATED.md";

/// File with the signature written at export time, read by the chat API (ADR-0094).
pub const SIGNATURE_FILE: &str = "agent-signature.json";

/// File that maps spec names to agent names, read by the chat API (ADR-0094).
pub const NAMES_FILE: &str = "agent-names.json";

/// The destinations of the agent tree, in the image's `app/` (ADR-0110).
pub const EGRESS_FILE: &str = "egress.json";

/// Copies what the service reads at run time to `app`: the chat API's signature files next to the
/// root spec, the destinations of the tree and the resolved agent tree (companion files, sub-agent
/// specs, skills, the tool cache of the sub-agents and the lockfile; ADR-0115). The directory
/// exists afterwards.
#[cfg(all(
    feature = "mcp",
    feature = "builtin",
    feature = "sandbox",
    feature = "a2a"
))]
pub fn copy_app_files(tree: &AgentTree, builder: &ToolBuilder, app: &Path) -> Result<(), String> {
    std::fs::create_dir_all(app).map_err(|error| format!("{}: {error}", app.display()))?;
    let base = tree.root().base_dir();
    for name in [SIGNATURE_FILE, NAMES_FILE] {
        if base.join(name).is_file() {
            copy_recursively(&base.join(name), &app.join(name))?;
        }
    }
    // The destinations of the tree, for the deployment's network rules (ADR-0110, addendum B):
    // run and build phase, told apart by `phase`.
    let credentials = grauwolf_egress::credentials_file(tree);
    let destinations = grauwolf_egress::derive_tree(
        tree,
        &grauwolf_egress::Options {
            credentials_toml: credentials.as_deref(),
            otel: false,
        },
    );
    let target = app.join(EGRESS_FILE);
    std::fs::write(&target, grauwolf_egress::to_json(&destinations))
        .map_err(|error| format!("{}: {error}", target.display()))?;
    tree.copy_into(app, builder)
}

/// Writes the files of a generated crate into `out`, creating folders as needed.
pub fn write_crate(generated: &GeneratedCrate, out: &Path) -> Result<(), String> {
    let text = generated
        .files
        .iter()
        .map(|(path, content)| (path, content.as_bytes()));
    let binary = generated
        .binary_files
        .iter()
        .map(|(path, content)| (path, content.as_slice()));
    for (path, content) in text.chain(binary) {
        let target = out.join(path);
        let write = |target: &Path| -> std::io::Result<()> {
            std::fs::create_dir_all(target.parent().unwrap_or(out))?;
            std::fs::write(target, content)
        };
        write(&target).map_err(|error| format!("{}: {error}", target.display()))?;
    }
    Ok(())
}

#[cfg(all(
    feature = "mcp",
    feature = "builtin",
    feature = "sandbox",
    feature = "a2a"
))]
/// A harness project that was written.
#[derive(Debug, Clone)]
pub struct HarnessProject {
    /// The harness file as read.
    pub harness: Harness,
    /// The project folder, `<out_dir>/<name>`.
    pub project: PathBuf,
    /// Warnings of the agent tree (unused files, …).
    pub warnings: Vec<String>,
}

/// Generates the project of the harness file `file` into `<out_dir>/<name>/` (ADR-0128): the
/// service crate of its root spec with the `Dockerfile` for the builder image of this version,
/// `app/` (see [`copy_app_files`]) and [`NOTE_FILE`]. An earlier project there is removed first –
/// but only after the harness file was read without errors. The agent tree is resolved with
/// `builder` (its tool cache) and `locked`, so that the image build needs neither network nor
/// toolchain.
#[cfg(all(
    feature = "mcp",
    feature = "builtin",
    feature = "sandbox",
    feature = "a2a"
))]
pub async fn generate_harness(
    file: &Path,
    out_dir: &Path,
    builder: &ToolBuilder,
    locked: bool,
) -> Result<HarnessProject, String> {
    let json =
        std::fs::read_to_string(file).map_err(|error| format!("{}: {error}", file.display()))?;
    let harness = Harness::parse_named(&file.display().to_string(), &json)?;
    let spec_file = file.parent().unwrap_or(Path::new(".")).join(&harness.spec);
    let project = out_dir.join(&harness.name);
    // The folder is output only (ADR-0128 decision 3): a file the generator no longer writes must
    // not survive.
    match std::fs::remove_dir_all(&project) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!("{}: {error}", project.display()));
        }
        _ => {}
    }
    let mut tree = crate::agent_tree::read(&spec_file).map_err(|errors| errors.to_string())?;
    let config = crate::config::RegistryConfig::from_env(&crate::config::ProcessEnv);
    tree.resolve(&ResolveOptions {
        builder,
        locked,
        config: &config,
        build_code: true,
        resolve_root: true,
    })
    .await?;
    let spec = tree.root().image_spec();
    let resolution = tree.root().resolution().expect("the tree is resolved");
    let version = env!("CARGO_PKG_VERSION");
    let generated = grauwolf_generator::generate_crate_with(
        &spec,
        &grauwolf_generator::GrauwolfDependency::Path(PathBuf::from(BUILDER_CRATES)),
        &resolution.artifacts,
        &grauwolf_generator::GenerateOptions {
            package_tools: resolution.package_tools.clone(),
            service: true,
            full_facade: true,
            tool_sources: None,
            dockerfile: Some(grauwolf_generator::DockerfileOptions {
                builder_image: grauwolf_generator::default_builder_image(version),
                expected_builder_version: Some(version.to_owned()),
            }),
        },
    )
    .map_err(|error| format!("{}: {error}", spec_file.display()))?;
    write_crate(&generated, &project)?;
    copy_app_files(&tree, builder, &project.join(grauwolf_generator::APP_DIR))?;
    let note = project.join(NOTE_FILE);
    let text = format!(
        "# Generated project\n\nGenerated from the harness `{}` (ADR-0128). Do not edit: \
         grauwolf harness generate replaces this folder completely. Change the harness file or \
         its specs instead.\n",
        file.display()
    );
    std::fs::write(&note, text).map_err(|error| format!("{}: {error}", note.display()))?;
    Ok(HarnessProject {
        harness,
        project,
        warnings: tree.warnings().to_vec(),
    })
}

#[cfg(all(
    feature = "mcp",
    feature = "builtin",
    feature = "sandbox",
    feature = "a2a"
))]
fn copy_recursively(source: &Path, target: &Path) -> Result<(), String> {
    let fail = |error: std::io::Error| format!("{}: {error}", source.display());
    if source.is_dir() {
        std::fs::create_dir_all(target).map_err(fail)?;
        for entry in std::fs::read_dir(source).map_err(fail)? {
            let entry = entry.map_err(fail)?;
            copy_recursively(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else {
        std::fs::create_dir_all(target.parent().unwrap_or(Path::new("."))).map_err(fail)?;
        std::fs::copy(source, target).map_err(fail)?;
    }
    Ok(())
}
