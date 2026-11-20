//! Graph of a spec as a Mermaid diagram (F13.14).

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;

use grauwolf_spec::AgentSpec;

fn example(name: &str) -> AgentSpec {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/specs")
        .join(name);
    AgentSpec::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn draws_nodes_with_their_kind_and_edges_with_their_condition() {
    assert_eq!(
        example("calculator.json").to_mermaid(),
        "flowchart TD\n\
         \x20   start([start])\n\
         \x20   finish([finish])\n\
         \x20   call_llm[\"call_llm<br/><small>llm_request</small>\"]\n\
         \x20   execute_tool[\"execute_tool<br/><small>execute_tool</small>\"]\n\
         \x20   send_result[\"send_result<br/><small>llm_send_tool_result</small>\"]\n\
         \x20   execute_tools_in_order[\"execute_tools_in_order<br/><small>execute_multiple_tools</small>\"]\n\
         \x20   send_results[\"send_results<br/><small>llm_send_multiple_tool_results</small>\"]\n\
         \x20   start --> call_llm\n\
         \x20   call_llm -->|on_tool_call| execute_tool\n\
         \x20   call_llm -->|on_multiple_tool_calls| execute_tools_in_order\n\
         \x20   call_llm -->|on_assistant_message| finish\n\
         \x20   execute_tool --> send_result\n\
         \x20   send_result -->|on_tool_call| execute_tool\n\
         \x20   send_result -->|on_multiple_tool_calls| execute_tools_in_order\n\
         \x20   send_result -->|on_assistant_message| finish\n\
         \x20   execute_tools_in_order --> send_results\n\
         \x20   send_results -->|on_tool_call| execute_tool\n\
         \x20   send_results -->|on_multiple_tool_calls| execute_tools_in_order\n\
         \x20   send_results -->|on_assistant_message| finish\n"
    );
}

#[test]
fn draws_subgraphs_as_their_own_blocks_linked_from_the_calling_node() {
    let diagram = example("planner.json").to_mermaid();

    assert!(
        diagram.contains("    solve[\"solve<br/><small>plan</small>\"]\n"),
        "{diagram}"
    );
    assert!(
        diagram.contains("    subgraph sg_step [\"Subgraph step\"]\n"),
        "{diagram}"
    );
    assert!(
        diagram.contains("        step__call_llm[\"call_llm<br/><small>llm_request</small>\"]\n"),
        "{diagram}"
    );
    assert!(
        diagram.contains("        step__start --> step__call_llm\n"),
        "{diagram}"
    );
    assert!(diagram.contains("    solve -.-> sg_step\n"), "{diagram}");
}
