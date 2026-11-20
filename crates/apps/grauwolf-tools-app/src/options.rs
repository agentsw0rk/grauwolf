//! Option structs shared by the commands: where packages are cached and how tool code is built.

use std::path::PathBuf;

use grauwolf_toolbuild::ToolBuilder;

/// Default cache for built tools, relative to the working directory.
pub const DEFAULT_CACHE: &str = ".grauwolf/tools";

/// Where fetched tool packages are cached.
#[derive(clap::Args)]
pub struct CacheOptions {
    /// Cache directory (packages land in its `packages/` folder).
    #[arg(long, default_value = DEFAULT_CACHE)]
    pub cache: PathBuf,
}

/// Options for building tool code (ADR-0030).
#[derive(clap::Args)]
pub struct BuildOptions {
    /// Cache directory for guest crates and artifacts.
    #[arg(long, default_value = DEFAULT_CACHE)]
    pub cache: PathBuf,
    /// Tool packages must be in `grauwolf.lock`: fails instead of changing the lockfile
    /// (CI, images; ADR-0107).
    #[arg(long)]
    pub locked: bool,
    /// Prefix under which cargo runs (e.g. `bwrap …` or `docker run …`); give each part
    /// once: `--build-wrapper bwrap --build-wrapper --unshare-net`.
    #[arg(long = "build-wrapper", allow_hyphen_values = true)]
    pub wrapper: Vec<String>,
}

impl BuildOptions {
    pub fn tool_builder(&self) -> ToolBuilder {
        tool_builder(&self.cache, &self.wrapper)
    }
}

/// The tool builder on `cache`, with `wrapper` as the prefix for cargo if it is not empty.
pub fn tool_builder(cache: &std::path::Path, wrapper: &[String]) -> ToolBuilder {
    let builder = ToolBuilder::new(cache);
    if wrapper.is_empty() {
        builder
    } else {
        builder.with_wrapper(wrapper.to_vec())
    }
}
