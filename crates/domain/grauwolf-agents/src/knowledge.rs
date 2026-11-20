//! Knowledge as context (ADR-0113): the best sections for the user input are added to the prompt
//! before the run, like the automatic recall of the long-term memory (ADR-0069).

use grauwolf_prompt::Message;
use grauwolf_tools::KnowledgeContext;

use crate::AgentContext;
use crate::events::AgentEvent;

/// Appends the best sections for `input` as a system message that names their sources and emits
/// `knowledge_injected`; without hits nothing happens, and an error is reported as
/// `knowledge_failed` without ending the run.
pub async fn inject_knowledge(
    ctx: &mut AgentContext,
    knowledge: &dyn KnowledgeContext,
    input: &str,
) {
    match knowledge.retrieve(input).await {
        Ok(hits) if hits.is_empty() => {}
        Ok(hits) => {
            let excerpts: Vec<String> = hits
                .iter()
                .map(|hit| format!("[{}] {}", hit.source, hit.text))
                .collect();
            ctx.append(Message::system(format!(
                "Relevant excerpts from the knowledge base. Use them when they help to answer, \
                 and name the source when you rely on one:\n\n{}",
                excerpts.join("\n\n")
            )));
            let mut sources: Vec<String> = Vec::new();
            for hit in &hits {
                if !sources.contains(&hit.source) {
                    sources.push(hit.source.clone());
                }
            }
            ctx.emit(AgentEvent::KnowledgeInjected {
                sources,
                count: hits.len(),
            });
        }
        Err(error) => ctx.emit(AgentEvent::KnowledgeFailed {
            error: error.to_string(),
        }),
    }
}
