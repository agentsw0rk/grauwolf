//! System prompt of the builder (ADR-0035): schema, catalogs, rules for tool code, example.

use grauwolf_graph::{Catalogs, DataType};
use grauwolf_prompt::LlModel;
use grauwolf_spec::AgentSpec;
use serde_json::Value;

/// Example spec with tool loop and tool code (few-shot).
const EXAMPLE: &str = include_str!("../../../../examples/specs/calculator.json");

/// Instructions for the LLM, generated from the built-in catalogs.
pub(crate) fn system_prompt(agent_model: Option<&LlModel>) -> String {
    let catalogs = Catalogs::builtin();
    let model = match agent_model {
        Some(model) => format!(
            "- Set `model` to {{\"provider\": \"{}\", \"id\": \"{}\"}}.\n",
            model.provider, model.id
        ),
        None => String::new(),
    };
    format!(
        "You generate AgentSpecs for grauwolf: JSON documents from which a \
         runnable GraphAgent is built. Answer with nothing but the JSON of the complete \
         spec, without explanations.\n\
         \n\
         ## Structure\n\
         - A graph has nodes and edges; `start` and `finish` exist implicitly and are forbidden as \
         node IDs. IDs and names are snake_case (`[a-z][a-z0-9_]*`), type names PascalCase.\n\
         - Edges are type-checked: output of the source → (condition) → input of the target. `start` \
         supplies the type from `input`, `finish` expects the type from `output` (default `string`).\n\
         - If a node has several outgoing edges, the first one whose condition matches wins.\n\
         - `finish` and all nodes must be reachable from `start`.\n\
         {model}\
         \n\
         ## Node kinds (`kind`, signature input → output)\n\
         {nodes}\n\
         - subgraph, params {{\"subgraph\":\"<name>\"}}: embeds an entry from `subgraphs`; \
         signature = its `input` → `output`; `tools` restricts the tools in it\n\
         - parallel, params {{\"subgraphs\":[\"<a>\",\"<b>\"],\"merge\":\"concat\"}}: \
         runs several subgraphs of the same signature at the same time; `merge` is `concat` \
         (text results as sections), `index` (with `index`: result of that branch) or \
         `llm_select` (optional `criteria`: the LLM picks the best branch); signature = that of the \
         subgraphs\n\
         - plan, params {{\"subgraph\":\"<name>\",\"critic\":true,\"max_iterations\":10}}: \
         string → string; an LLM plans the task in steps, the \
         subgraph (string → string, e.g. with a tool loop) carries out each step; `critic` has the plan checked before \
         every further step; only `subgraph` is required\n\
         \n\
         ## Conditions (`when.kind`, signature edge value → forwarded value)\n\
         {conditions}\n\
         \n\
         ## Tool code\n\
         `code` is the body of `fn tool(args: serde_json::Value) -> Result<String, String>`. \
         Only `serde_json` is available; `include!`, `include_str!`, \
         `include_bytes!`, `env!`, `option_env!`, `mod`, `extern`, `unsafe` and attributes (`#`) are forbidden. \
         The code returns errors as `Err(text)`, the result as `Ok(text)`. `args` \
         describes the argument schema of the tool for the LLM. In the JSON, `code` is a string: \
         line breaks in it are `\\n` with exactly one backslash, quotation marks `\\\"`.\n\
         \n\
         ## Proven patterns\n\
         Adopt one of these patterns and change only what the requirement demands.\n\
         Without tools (node `call_llm` of type llm_request):\n\
         start → call_llm\n\
         call_llm → finish [on_assistant_message]\n\
         With tools, simplest: no `graph`, instead `\"strategy\": {{\"kind\": \"single_run\"}}`; it expands \
         into the tool loop below, including the several-calls branch.\n\
         With tools, hand-written (call_llm: llm_request, execute_tool: execute_tool, \
         execute_all: execute_multiple_tools with params {{\"sequential\":true}}, send_result: \
         llm_send_tool_result, send_all: llm_send_multiple_tool_results). Models sometimes answer \
         with several tool calls at once; `on_tool_call` matches exactly one call, so every node \
         that routes `on_tool_call` also needs `on_multiple_tool_calls`, declared right after it \
         (the first matching edge wins):\n\
         start → call_llm\n\
         call_llm → execute_tool [on_tool_call]\n\
         call_llm → execute_all [on_multiple_tool_calls]\n\
         call_llm → finish [on_assistant_message]\n\
         execute_tool → send_result\n\
         execute_all → send_all\n\
         send_result → execute_tool [on_tool_call]\n\
         send_result → execute_all [on_multiple_tool_calls]\n\
         send_result → finish [on_assistant_message]\n\
         send_all → execute_tool [on_tool_call]\n\
         send_all → execute_all [on_multiple_tool_calls]\n\
         send_all → finish [on_assistant_message]\n\
         Only `llm_request` and `llm_send_tool_result` supply `message`; only conditions \
         turn it into text (`on_assistant_message`) or tool calls (`on_tool_call`, `on_multiple_tool_calls`). \
         Use `llm_request_structured` only with a type from `types`. Omit `when` if no \
         condition is needed.\n\
         \n\
         ## JSON schema of the spec\n\
         {schema}\n\
         \n\
         ## Example\n\
         {EXAMPLE}",
        nodes = node_lines(&catalogs),
        conditions = condition_lines(&catalogs),
        schema = AgentSpec::json_schema(),
    )
}

/// `- <kind>[, params <example>]: <input> → <output>` per node kind.
fn node_lines(catalogs: &Catalogs) -> String {
    catalogs
        .nodes
        .kinds()
        .filter_map(|kind| {
            let descriptor = catalogs.nodes.get(kind)?;
            let params = descriptor.example_params();
            let (input, output) = descriptor.signature(&params).ok()?;
            let params = match params {
                Value::Null => String::new(),
                params => format!(", params {params}"),
            };
            Some(format!("- {kind}{params}: {input} → {output}"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `- <condition>[, params <example>]: <input> → <output>` per condition, for the matching
/// input; `Type` stands for a type from `types`.
fn condition_lines(catalogs: &Catalogs) -> String {
    let inputs = [
        DataType::String,
        DataType::Message,
        DataType::ToolCall,
        DataType::ToolResult,
        DataType::ToolCalls,
        DataType::ToolResults,
        DataType::Named("Type".into()),
    ];
    catalogs
        .conditions
        .kinds()
        .filter_map(|kind| {
            let descriptor = catalogs.conditions.get(kind)?;
            let params = descriptor.example_params();
            inputs.iter().find_map(|input| {
                let output = descriptor.signature(&params, input).ok()?;
                let params = match &params {
                    Value::Null => String::new(),
                    params => format!(", params {params}"),
                };
                Some(format!("- {kind}{params}: {input} → {output}"))
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}
