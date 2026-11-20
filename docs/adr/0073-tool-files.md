# ADR-0073: Tool code and tool definitions in separate files

- **Status:** Accepted (User, 2026-11-09: "yes, do 1 and 2")
- **Date:** 2026-11-09

## Context
Tool code is a JSON string in the spec (`tools[].code`, ADR-0024): line breaks and quotation marks must be escaped, there is no syntax highlighting, and the same tool must be copied into every spec. The user wants to create small tools and reference them without adding them to the project.

## Decision
- `tools[].code_file`: path (relative to the file that names it) to a file with the code body, instead of `code`; exactly one of the two.
- `tool_files`: list of tool definitions in their own JSON files (relative to the spec), each in the format of an entry of `tools` (there, `code_file` is relative to the tool file). They are appended to `tools`; this way, a tool library can be used in several specs.
- The references are resolved when the spec is loaded in the facade (`grauwolf::loader::load_spec`), before it is checked; domain crates read no files. Afterwards it is an ordinary spec: checking, tool build, sandbox, cache, interpreter and generator remain unchanged. Errors name the spec, the JSON pointer and the file.
- `AgentSpec::from_json` does not know the references; whoever reads a spec without the loader must give the code inline.

## Consequences
- Tool code can be written, checked and versioned as a normal `.rs` file and used in several specs.
- Compile errors still point to `/tools/<i>/code` with the line in the code body, not to the file name.
