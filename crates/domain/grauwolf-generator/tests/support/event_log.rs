/// Records all events as JSON, for comparison between the interpreter and the generated crate.
#[derive(Default)]
pub(crate) struct EventLog(std::sync::Mutex<Vec<serde_json::Value>>);

impl EventLog {
    pub(crate) fn events(&self) -> serde_json::Value {
        serde_json::Value::Array(self.0.lock().unwrap().clone())
    }
}

impl grauwolf_agents::events::Feature for EventLog {
    fn on_event(&self, event: &grauwolf_agents::events::AgentEvent) {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::to_value(event).unwrap());
    }
}
