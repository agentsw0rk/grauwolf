//! Condition catalog (ADR-0021).

use grauwolf_graph::{CatalogError, ConditionCatalog, DataType};
use serde_json::Value;

#[test]
fn on_assistant_message_turns_a_message_into_its_text() {
    let catalog = ConditionCatalog::builtin();
    let condition = catalog
        .get("on_assistant_message")
        .expect("on_assistant_message missing");

    assert_eq!(
        condition.signature(&Value::Null, &DataType::Message),
        Ok(DataType::String)
    );
    assert!(matches!(
        condition.signature(&Value::Null, &DataType::String),
        Err(CatalogError::InputTypeMismatch { .. })
    ));
}
