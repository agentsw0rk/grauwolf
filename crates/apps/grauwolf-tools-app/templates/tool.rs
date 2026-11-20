// The code body of the tool `{{tool}}`: it becomes
// `fn tool(args: serde_json::Value) -> Result<String, String> { … }` and runs in the sandbox
// (no files, no network). `args` follows the schema in grauwolf-tools.json.
// This is only an example (it upper-cases `input`): replace it with what `{{tool}}` does, and
// describe the arguments in grauwolf-tools.json.
let input = args["input"].as_str().ok_or("input is missing")?;
Ok(input.to_uppercase())
