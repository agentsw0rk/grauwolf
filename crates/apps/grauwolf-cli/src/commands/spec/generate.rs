//! `grauwolf spec generate <file> --out <dir>`: a standalone Rust crate from the spec, optionally
//! with the service, a Dockerfile and a `cargo check` (ADR-0084, ADR-0093, ADR-0096).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use grauwolf_generator::GrauwolfDependency;
use grauwolf_spec::AgentSpecExt;
use serde_json::Value;

use super::build::resolve_read_tree;
use crate::options::BuildOptions;
use crate::output::{Io, outln, report};

use grauwolf_host::project::BUILDER_CRATES;

/// Options of `spec generate` besides file, output and build options.
pub struct GenerateArgs {
    pub grauwolf_crates: Option<PathBuf>,
    pub check: bool,
    pub service: bool,
    pub full_facade: bool,
    pub dockerfile: bool,
    pub builder_image: Option<String>,
    pub allow_builder_mismatch: bool,
    pub tool_sources: bool,
}

/// `grauwolf spec generate <file> --out <dir> [--cache <dir>] [--grauwolf-crates <dir>]`.
pub fn run(
    io: &Io,
    file: &Path,
    out: &Path,
    options: &BuildOptions,
    args: GenerateArgs,
) -> ExitCode {
    let GenerateArgs {
        grauwolf_crates,
        check,
        service,
        full_facade,
        dockerfile,
        builder_image,
        allow_builder_mismatch,
        tool_sources,
    } = args;
    report(
        io,
        (|| {
            // For the image the crates are where the builder keeps them, not on this machine.
            let crates = match (grauwolf_crates, dockerfile) {
                (Some(crates), _) => crates
                    .canonicalize()
                    .map_err(|error| format!("{}: {error}", crates.display()))?,
                (None, true) => PathBuf::from(BUILDER_CRATES),
                (None, false) => Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .canonicalize()
                    .map_err(|error| error.to_string())?,
            };
            let tree =
                grauwolf_host::agent_tree::read(file).map_err(|errors| errors.to_string())?;
            if tool_sources {
                if check {
                    return Err(
                        "--check needs the tool modules: run `grauwolf tools build` in \
                                the generated crate first, or generate without --tool-sources"
                            .to_owned(),
                    );
                }
                if let Some(package) = tree.root().spec().tool_packages.first() {
                    // Modules cannot be written as text. Git packages would have to be handed
                    // over as sources, which the resolver does not offer yet.
                    let reason = if package.oci.is_some() {
                        "OCI tool packages carry built modules; generate without --tool-sources \
                         or use a git source"
                    } else {
                        "git tool packages are not yet written as sources; generate without \
                         --tool-sources"
                    };
                    return Err(format!("{}: {reason}", file.display()));
                }
            }
            // The whole agent tree is resolved (ADR-0115): sub-agents' tool packages and code
            // tools are built now, so that the image needs neither network nor toolchain.
            let tree = resolve_read_tree(io, tree, options, |tree| {
                !tool_sources
                    || (dockerfile
                        && tree.nodes().iter().skip(1).any(|node| {
                            !node.spec().tools.is_empty() || !node.spec().code_pieces().is_empty()
                        }))
            })?;
            // Skills are copied into the image (ADR-0103): git sources are fetched now and the
            // spec in the crate names their copies, so the image needs no git.
            if !dockerfile
                && tree
                    .root()
                    .spec()
                    .skills
                    .iter()
                    .any(|source| source.git.is_some())
            {
                return Err(format!(
                    "{}: git `skills` are copied into the image: generate with --dockerfile",
                    file.display()
                ));
            }
            let spec = tree.root().image_spec();
            let resolution = tree.root().resolution().expect("the tree is resolved");
            // Tool sources (ADR-0095): the guest crates as text, nothing is compiled here.
            let (artifacts, package_tools, sources) = if tool_sources {
                let mut sources = spec
                    .tools
                    .iter()
                    .map(grauwolf_toolbuild::guest_crate)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| format!("{}: {error}", file.display()))?;
                // The code of the graphs is written as sources like the tools (ADR-0102).
                for piece in spec.code_pieces() {
                    sources.push(grauwolf_toolbuild::code_guest_crate(&piece).map_err(
                        |error| format!("{}:{}: {error}", file.display(), piece.pointer),
                    )?);
                }
                (Vec::new(), Vec::new(), Some(sources))
            } else {
                // Tool packages are fetched now and embedded like local tools (ADR-0107), so the
                // image build stays offline.
                (
                    resolution.artifacts.clone(),
                    resolution.package_tools.clone(),
                    None,
                )
            };
            let generated = grauwolf_generator::generate_crate_with(
                &spec,
                &GrauwolfDependency::Path(crates),
                &artifacts,
                &grauwolf_generator::GenerateOptions {
                    package_tools,
                    service,
                    full_facade: full_facade || dockerfile,
                    tool_sources: sources,
                    dockerfile: dockerfile.then(|| grauwolf_generator::DockerfileOptions {
                        builder_image: builder_image.unwrap_or_else(|| {
                            grauwolf_generator::default_builder_image(env!("CARGO_PKG_VERSION"))
                        }),
                        expected_builder_version: (!allow_builder_mismatch)
                            .then(|| env!("CARGO_PKG_VERSION").to_owned()),
                    }),
                },
            )
            .map_err(|error| format!("{}: {error}", file.display()))?;
            grauwolf_host::project::write_crate(&generated, out)?;
            if dockerfile {
                grauwolf_host::project::copy_app_files(
                    &tree,
                    &options.tool_builder(),
                    &out.join(grauwolf_generator::APP_DIR),
                )?;
            }
            outln!(io, "{}: crate written to {}", file.display(), out.display());
            if check {
                cargo_check(io, file, out, service || dockerfile)?;
            }
            Ok(())
        })(),
    )
}

/// `cargo check` of the generated crate in `out` (F13.9): errors with file, line and column
/// in the crate; without errors a confirmation.
fn cargo_check(io: &Io, file: &Path, out: &Path, service: bool) -> Result<(), String> {
    // The feature `service` includes `app`; this way everything is checked in one run.
    let feature = if service { "service" } else { "app" };
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = std::process::Command::new(cargo)
        .args([
            "check",
            "--quiet",
            "--message-format=json",
            "--features",
            feature,
            "--manifest-path",
        ])
        .arg(out.join("Cargo.toml"))
        .output()
        .map_err(|error| format!("cannot run cargo check: {error}"))?;
    let errors: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-message")
        .map(|message| message["message"].clone())
        .filter(|message| message["level"] == "error")
        .map(|message| {
            let span = message["spans"]
                .as_array()
                .and_then(|spans| spans.iter().find(|span| span["is_primary"] == true))
                .cloned()
                .unwrap_or(Value::Null);
            format!(
                "{}: {}:{}:{}: error: {}",
                file.display(),
                out.join(span["file_name"].as_str().unwrap_or("?"))
                    .display(),
                span["line_start"],
                span["column_start"],
                message["message"].as_str().unwrap_or_default()
            )
        })
        .collect();
    if output.status.success() {
        outln!(
            io,
            "{}: cargo check (with feature {feature}): no errors",
            file.display()
        );
        Ok(())
    } else if errors.is_empty() {
        Err(format!(
            "{}: cargo check failed:\n{}",
            file.display(),
            String::from_utf8_lossy(&output.stderr)
        ))
    } else {
        Err(errors.join("\n"))
    }
}
