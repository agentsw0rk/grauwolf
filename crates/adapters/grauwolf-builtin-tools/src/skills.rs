//! Skills (ADR-0103): folders with a `SKILL.md` (front matter `name` and `description`, then the
//! instructions) and optional files. [`SkillLibrary::load`] reads and checks the sources;
//! [`SkillLibrary::tools`] offers `load_skill` and `read_skill_file`, read-only. Scripts are never
//! run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_prompt::ToolDescriptor;
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

/// A skill file is at most this large (also the limit of `read_skill_file`).
pub const MAX_SKILL_FILE_BYTES: u64 = 1024 * 1024;

/// Name of the file with the instructions.
pub const SKILL_FILE: &str = "SKILL.md";

/// A directory with skills and the skills to take from it (`None`: all).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRoot {
    /// Directory with one sub-directory per skill.
    pub dir: PathBuf,
    /// Names of the skills to take; default: every sub-directory with a `SKILL.md`.
    pub names: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
struct Skill {
    description: String,
    instructions: String,
    dir: PathBuf,
    /// Files other than `SKILL.md`, relative to the skill folder, sorted.
    files: Vec<String>,
}

/// The skills of an agent, loaded and checked.
#[derive(Debug, Clone, Default)]
pub struct SkillLibrary {
    skills: BTreeMap<String, Skill>,
}

impl SkillLibrary {
    /// Reads and checks the skills of `roots`. Every skill needs a `SKILL.md` whose front matter
    /// has `name` (equal to the folder name) and `description`; names are unique across all
    /// roots, every file is at most [`MAX_SKILL_FILE_BYTES`] and no symlink leads out of its
    /// skill folder. Errors name the file.
    pub fn load(roots: &[SkillRoot]) -> Result<Self, String> {
        let mut skills: BTreeMap<String, Skill> = BTreeMap::new();
        for root in roots {
            let base = root
                .dir
                .canonicalize()
                .map_err(|error| format!("{}: {error}", root.dir.display()))?;
            let names = match &root.names {
                Some(names) => names.clone(),
                None => folders_with_skill_file(&base)?,
            };
            for name in names {
                let (name, skill) = load_skill(&base, &name)?;
                if skills.insert(name.clone(), skill).is_some() {
                    return Err(format!(
                        "skill '{name}' is defined more than once (found again in {})",
                        root.dir.display()
                    ));
                }
            }
        }
        Ok(Self { skills })
    }

    /// No skills?
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Names and descriptions, sorted by name.
    pub fn list(&self) -> Vec<(&str, &str)> {
        self.skills
            .iter()
            .map(|(name, skill)| (name.as_str(), skill.description.as_str()))
            .collect()
    }

    /// The block for the system prompt: the skills and the hint to call `load_skill`.
    pub fn prompt_block(&self) -> String {
        let mut text = String::from(
            "You have skills: folders of instructions that you load on demand. Available skills:\n",
        );
        for (name, description) in self.list() {
            text.push_str(&format!("- {name}: {description}\n"));
        }
        text.push_str(
            "When a task matches a skill, call `load_skill` with its name and follow the \
             instructions. Use `read_skill_file` for the other files a skill lists.",
        );
        text
    }

    /// The tools `load_skill` and `read_skill_file`.
    pub fn tools(&self) -> Vec<Arc<dyn DynTool>> {
        let library = Arc::new(self.clone());
        vec![
            Arc::new(SkillTool {
                library: Arc::clone(&library),
                read_file: false,
            }),
            Arc::new(SkillTool {
                library,
                read_file: true,
            }),
        ]
    }

    fn skill(&self, name: &str) -> Result<&Skill, ToolError> {
        self.skills.get(name).ok_or_else(|| {
            let known: Vec<&str> = self.skills.keys().map(String::as_str).collect();
            ToolError::Execution(format!(
                "unknown skill '{name}'; known skills: {}",
                known.join(", ")
            ))
        })
    }
}

fn folders_with_skill_file(base: &Path) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let entries =
        std::fs::read_dir(base).map_err(|error| format!("{}: {error}", base.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", base.display()))?;
        if entry.path().join(SKILL_FILE).is_file()
            && let Some(name) = entry.file_name().to_str()
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

fn load_skill(base: &Path, folder: &str) -> Result<(String, Skill), String> {
    let dir = base.join(folder);
    let file = dir.join(SKILL_FILE);
    let label = format!("{}", file.display());
    let dir = dir
        .canonicalize()
        .map_err(|_| format!("{label}: skill '{folder}' not found"))?;
    if dir.parent() != Some(base) {
        return Err(format!(
            "{label}: the skill folder leaves the skills directory"
        ));
    }
    let text = read_checked(&dir, &file, &label)?;
    let (front, instructions) = split_front_matter(&text)
        .ok_or_else(|| format!("{label}: starts without a front matter (`---` … `---`)"))?;
    let field = |key: &str| {
        front_value(front, key)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{label}: the front matter needs `{key}`"))
    };
    let name = field("name")?;
    let description = field("description")?;
    if name != folder {
        return Err(format!(
            "{label}: front matter name '{name}' differs from the folder name '{folder}'"
        ));
    }
    let mut files = Vec::new();
    collect_files(&dir, &dir, &mut files)?;
    files.sort();
    files.retain(|path| path != SKILL_FILE);
    Ok((
        name,
        Skill {
            description,
            instructions: instructions.trim_start_matches(['\r', '\n']).to_owned(),
            dir,
            files,
        },
    ))
}

/// Reads `file` (below `dir`) as text; checks size and symlinks.
fn read_checked(dir: &Path, file: &Path, label: &str) -> Result<String, String> {
    let bytes = read_inside(dir, file).map_err(|reason| format!("{label}: {reason}"))?;
    String::from_utf8(bytes).map_err(|_| format!("{label}: is not UTF-8 text"))
}

/// Bytes of `file` if it lies inside `dir` after resolving symlinks and is small enough.
fn read_inside(dir: &Path, file: &Path) -> Result<Vec<u8>, String> {
    let real = file.canonicalize().map_err(|error| error.to_string())?;
    if !real.starts_with(dir) {
        return Err("a symlink leads out of the skill folder".to_owned());
    }
    let meta = std::fs::metadata(&real).map_err(|error| error.to_string())?;
    if !meta.is_file() {
        return Err("is not a file".to_owned());
    }
    if meta.len() > MAX_SKILL_FILE_BYTES {
        return Err(format!(
            "is larger than {} bytes ({} bytes)",
            MAX_SKILL_FILE_BYTES,
            meta.len()
        ));
    }
    std::fs::read(&real).map_err(|error| error.to_string())
}

fn collect_files(dir: &Path, folder: &Path, files: &mut Vec<String>) -> Result<(), String> {
    let entries =
        std::fs::read_dir(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("{}: {error}", folder.display()))?
            .path();
        let real = path
            .canonicalize()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if !real.starts_with(dir) {
            return Err(format!(
                "{}: a symlink leads out of the skill folder",
                path.display()
            ));
        }
        if real.is_dir() {
            collect_files(dir, &path, files)?;
        } else {
            let size = std::fs::metadata(&real)
                .map_err(|error| format!("{}: {error}", path.display()))?
                .len();
            if size > MAX_SKILL_FILE_BYTES {
                return Err(format!(
                    "{}: is larger than {MAX_SKILL_FILE_BYTES} bytes ({size} bytes)",
                    path.display()
                ));
            }
            let relative = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            files.push(relative);
        }
    }
    Ok(())
}

/// Front matter and the rest of a `SKILL.md`.
fn split_front_matter(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("---")?.strip_prefix(['\n', '\r'])?;
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Some((&rest[..offset], &rest[offset + line.len()..]));
        }
        offset += line.len();
    }
    None
}

/// Value of `key: value` in the front matter, without surrounding quotes.
fn front_value(front: &str, key: &str) -> Option<String> {
    front.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.trim() == key).then(|| {
            value
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .to_owned()
        })
    })
}

struct SkillTool {
    library: Arc<SkillLibrary>,
    read_file: bool,
}

impl SkillTool {
    fn name_arg(&self, args: &Value, field: &str) -> Result<String, ToolError> {
        args[field]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ToolError::InvalidArgs {
                tool: DynTool::name(self).to_owned(),
                reason: format!("missing field '{field}' (text)"),
            })
    }

    fn load(&self, args: &Value) -> Result<String, ToolError> {
        let skill = self.library.skill(&self.name_arg(args, "name")?)?;
        let mut text = skill.instructions.clone();
        if !skill.files.is_empty() {
            text.push_str("\n\nFiles of this skill (read them with `read_skill_file`):\n");
            for file in &skill.files {
                text.push_str(&format!("- {file}\n"));
            }
        }
        Ok(text)
    }

    fn read(&self, args: &Value) -> Result<String, ToolError> {
        let skill = self.library.skill(&self.name_arg(args, "name")?)?;
        let path = self.name_arg(args, "path")?;
        let relative = Path::new(&path);
        if path.is_empty()
            || !relative
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            return Err(ToolError::Execution(format!(
                "path '{path}' must be relative to the skill folder, without `..`"
            )));
        }
        let bytes = read_inside(&skill.dir, &skill.dir.join(relative))
            .map_err(|reason| ToolError::Execution(format!("{path}: {reason}")))?;
        String::from_utf8(bytes).map_err(|_| {
            ToolError::Execution(format!(
                "{path} is a binary file; only text files can be read"
            ))
        })
    }
}

impl DynTool for SkillTool {
    fn name(&self) -> &str {
        if self.read_file {
            "read_skill_file"
        } else {
            grauwolf_tools::LOAD_SKILL_TOOL
        }
    }

    fn descriptor(&self) -> ToolDescriptor {
        let name = json!({ "type": "string", "description": "Name of the skill" });
        if self.read_file {
            ToolDescriptor {
                name: "read_skill_file".to_owned(),
                description: "Reads a text file of a skill, e.g. a reference or a template that \
                              `load_skill` listed. Scripts are not run."
                    .to_owned(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "name": name,
                        "path": { "type": "string",
                            "description": "Path of the file relative to the skill folder" }
                    },
                    "required": ["name", "path"]
                }),
            }
        } else {
            ToolDescriptor {
                name: grauwolf_tools::LOAD_SKILL_TOOL.to_owned(),
                description: "Loads the instructions of a skill and lists its other files. \
                              Call it when a task matches the description of a skill."
                    .to_owned(),
                parameters: json!({
                    "type": "object",
                    "properties": { "name": name },
                    "required": ["name"]
                }),
            }
        }
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            if self.read_file {
                self.read(&args)
            } else {
                self.load(&args)
            }
        })
    }

    fn prompt_addition(&self) -> Option<String> {
        (!self.read_file).then(|| self.library.prompt_block())
    }
}
