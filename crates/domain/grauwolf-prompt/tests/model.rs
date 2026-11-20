//! Sampling settings of a model (ADR-0097).

use grauwolf_prompt::LlModel;

#[test]
fn temperature_and_think_are_part_of_the_model_and_its_json() {
    let model = LlModel::new("ollama", "qwen")
        .with_temperature(0.5)
        .with_think(true);

    assert_eq!(model.temperature, Some(0.5));
    assert_eq!(model.think, Some(true));
    let json = serde_json::to_value(&model).unwrap();
    assert_eq!(json["temperature"], 0.5);
    assert_eq!(json["think"], true);
    assert_eq!(serde_json::from_value::<LlModel>(json).unwrap(), model);
}

#[test]
fn unset_settings_are_not_serialized_and_old_json_stays_readable() {
    let model = LlModel::new("openai", "gpt-4o");
    let json = serde_json::to_value(&model).unwrap();
    assert!(json.get("temperature").is_none() && json.get("think").is_none());
    let old = serde_json::json!({"provider": "openai", "id": "gpt-4o"});
    assert_eq!(serde_json::from_value::<LlModel>(old).unwrap(), model);
}

#[test]
fn models_with_different_temperatures_are_different_map_keys() {
    let a = LlModel::new("openai", "gpt-4o").with_temperature(0.0);
    let b = LlModel::new("openai", "gpt-4o").with_temperature(1.0);
    let set: std::collections::HashSet<_> = [a, b].into_iter().collect();
    assert_eq!(set.len(), 2);
}

#[test]
fn without_fallbacks_keeps_the_sampling_settings() {
    let model = LlModel::new("ollama", "q")
        .with_temperature(0.2)
        .with_fallbacks(vec![LlModel::new("openai", "x")]);
    let plain = model.without_fallbacks();
    assert!(plain.fallbacks.is_empty());
    assert_eq!(plain.temperature, Some(0.2));
}
