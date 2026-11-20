//! Local skill directories of a spec (ADR-0103): the check `spec validate` and the read phase
//! apply, and the turn of a git source into a local directory.

use std::path::Path;

use grauwolf_builtin_tools::{SkillLibrary, SkillRoot};
use grauwolf_spec::AgentSpec;

/// Checks the local skill directories of `spec` (relative to `base_dir`) like loading them does
/// (`SKILL.md`, names, sizes, symlinks); git sources are skipped, they are checked when they
/// are resolved. For `spec validate`.
pub fn check_local(spec: &AgentSpec, base_dir: &Path) -> Result<(), String> {
    let roots: Vec<SkillRoot> = spec
        .skills
        .iter()
        .filter_map(|source| {
            source.dir.as_ref().map(|dir| SkillRoot {
                dir: base_dir.join(dir),
                names: source.names.clone(),
            })
        })
        .collect();
    SkillLibrary::load(&roots).map(|_| ())
}

/// Turns the git source `source` into a local one at `dir`; `names` stay.
pub fn replace_with_dir(source: &mut grauwolf_spec::SkillSourceSpec, dir: String) {
    *source = grauwolf_spec::SkillSourceSpec {
        dir: Some(dir),
        git: None,
        path: None,
        rev: None,
        tag: None,
        branch: None,
        names: source.names.take(),
    };
}
