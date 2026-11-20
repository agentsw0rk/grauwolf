//! Skills of a spec (ADR-0103): the tools `load_skill` and `read_skill_file` over the skill
//! folders. Local directories are read relative to the spec; git sources have to be turned into
//! local directories first (the loader and the generator do that, see ADR-0103).

use std::path::Path;
use std::sync::Arc;

use grauwolf_builtin_tools::{SkillLibrary, SkillRoot};
use grauwolf_spec::AgentSpec;
use grauwolf_tools::DynTool;

/// The skill roots of the local sources of `spec`, relative to `base_dir`. A git source is an
/// error here: it must have been resolved into a local directory before.
pub fn roots(spec: &AgentSpec, base_dir: &Path) -> Result<Vec<SkillRoot>, String> {
    spec.skills
        .iter()
        .enumerate()
        .map(|(index, source)| match &source.dir {
            Some(dir) => Ok(SkillRoot {
                dir: base_dir.join(dir),
                names: source.names.clone(),
            }),
            None => Err(format!(
                "/skills/{index}: git skills are not resolved to a local directory"
            )),
        })
        .collect()
}

/// Loads and checks the skills of `spec`.
pub fn library(spec: &AgentSpec, base_dir: &Path) -> Result<SkillLibrary, String> {
    SkillLibrary::load(&roots(spec, base_dir)?).map_err(|error| format!("skills: {error}"))
}

/// `load_skill` and `read_skill_file` for `spec`; none without `skills`.
pub(crate) fn tools(spec: &AgentSpec, base_dir: &Path) -> Result<Vec<Arc<dyn DynTool>>, String> {
    if spec.skills.is_empty() {
        return Ok(Vec::new());
    }
    Ok(library(spec, base_dir)?.tools())
}

pub use grauwolf_agent_tree::{check_local, replace_with_dir};
