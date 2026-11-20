//! Graph of a spec as a Mermaid diagram (F13.14): the main graph, subgraphs as their own blocks
//! (`sg_<name>`, nodes inside them prefixed `<name>__`), dashed arrows from nodes that
//! include subgraphs.

use std::fmt::Write;

use serde_json::Value;

use crate::{
    AgentSpec, FOR_EACH_KIND, ForEachBody, ForEachParams, GOAP_KIND, GraphSpec, PARALLEL_KIND,
    PLAN_KIND, SUBGRAPH_KIND,
};
use crate::{FINISH, START};

impl AgentSpec {
    /// Mermaid flowchart of the agent (`flowchart TD`).
    pub fn to_mermaid(&self) -> String {
        let mut out = String::from("flowchart TD\n");
        draw(&mut out, &self.graph, "", "    ");
        for (name, subgraph) in &self.subgraphs {
            let _ = writeln!(out, "    subgraph sg_{name} [\"Subgraph {name}\"]");
            draw(&mut out, &subgraph.graph, &format!("{name}__"), "        ");
            out.push_str("    end\n");
        }
        for (prefix, graph) in std::iter::once((String::new(), &self.graph)).chain(
            self.subgraphs
                .iter()
                .map(|(name, subgraph)| (format!("{name}__"), &subgraph.graph)),
        ) {
            for node in &graph.nodes {
                for called in called(node.kind.as_str(), node.params.as_ref()) {
                    let _ = writeln!(out, "    {prefix}{} -.-> sg_{called}", node.id);
                }
            }
        }
        out
    }
}

/// Nodes and edges of `graph`; IDs get `prefix` so that subgraphs stay unique.
fn draw(out: &mut String, graph: &GraphSpec, prefix: &str, indent: &str) {
    let _ = writeln!(out, "{indent}{prefix}{START}([{START}])");
    let _ = writeln!(out, "{indent}{prefix}{FINISH}([{FINISH}])");
    for node in &graph.nodes {
        let _ = writeln!(
            out,
            "{indent}{prefix}{id}[\"{id}<br/><small>{kind}</small>\"]",
            id = node.id,
            kind = node.kind
        );
    }
    for edge in &graph.edges {
        let label = edge
            .when
            .as_ref()
            .map(|when| format!("|{}|", when.kind))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "{indent}{prefix}{} -->{label} {prefix}{}",
            edge.from, edge.to
        );
    }
}

/// Subgraphs that a node includes.
fn called(kind: &str, params: Option<&Value>) -> Vec<String> {
    let Some(params) = params else {
        return Vec::new();
    };
    match kind {
        SUBGRAPH_KIND | PLAN_KIND => params["subgraph"]
            .as_str()
            .map(|name| vec![name.to_owned()])
            .unwrap_or_default(),
        PARALLEL_KIND => params["subgraphs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        // Each action of a GOAP node runs its subgraph (ADR-0070).
        GOAP_KIND => params["actions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|action| action["subgraph"].as_str())
            .map(str::to_owned)
            .collect(),
        FOR_EACH_KIND => match ForEachParams::parse(Some(params)).map(|parsed| parsed.body) {
            Ok(ForEachBody::Subgraph(name)) => vec![name],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}
