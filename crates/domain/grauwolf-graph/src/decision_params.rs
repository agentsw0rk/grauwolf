//! Parameters of the node `decide` and the condition `on_decision` (ADR-0100).

use serde_json::Value;

/// Number of options or levels a decision may have (the System One limit).
const MAX_ANSWERS: usize = 255;

/// Checks the parameters of `decide`; the error is `<pointer>: <reason>`.
pub(crate) fn check_decide(params: &Value) -> Result<(), String> {
    let at = |pointer: &str, reason: &str| format!("{pointer}: {reason}");
    let kind = params["kind"].as_str().unwrap_or_default();
    if !["choice", "yes_no", "score"].contains(&kind) {
        return Err(at("/kind", "must be \"choice\", \"yes_no\" or \"score\""));
    }
    if params["question"].as_str().is_none_or(str::is_empty) {
        return Err(at("/question", "must not be empty"));
    }
    // Every kind has its own parameters.
    let own: &[&str] = match kind {
        "choice" => &["options"],
        "score" => &["levels"],
        _ => &["threshold", "yes", "no"],
    };
    for name in ["options", "levels", "threshold", "yes", "no"] {
        if params.get(name).is_some() && !own.contains(&name) {
            return Err(at(
                &format!("/{name}"),
                &format!("does not apply to \"{kind}\""),
            ));
        }
    }
    match kind {
        "choice" => check_options(params.get("options"))?,
        "score" => check_levels(params.get("levels"))?,
        _ => {
            if let Some(threshold) = params.get("threshold")
                && !threshold.as_f64().is_some_and(|t| (0.0..=1.0).contains(&t))
            {
                return Err(at("/threshold", "must be between 0 and 1"));
            }
        }
    }
    if params.get("temperature").is_some_and(|t| !t.is_number()) {
        return Err(at("/temperature", "must be a number"));
    }
    if params.get("think").is_some_and(|t| !t.is_boolean()) {
        return Err(at("/think", "must be true or false"));
    }
    match params.get("decider").and_then(Value::as_str) {
        None | Some("llm") => {
            if let Some(model) = params.get("model").and_then(Value::as_str)
                && !model
                    .split_once('/')
                    .is_some_and(|(p, id)| !p.is_empty() && !id.is_empty())
            {
                return Err(at(
                    "/model",
                    "the \"llm\" decider needs a model written provider/id",
                ));
            }
        }
        Some("system_one") => {}
        Some(_) => return Err(at("/decider", "must be \"system_one\" or \"llm\"")),
    }
    Ok(())
}

fn check_options(options: Option<&Value>) -> Result<(), String> {
    let list = options
        .and_then(Value::as_array)
        .ok_or("/options: needs a list of options")?;
    check_count("/options", list.len())?;
    let mut names = Vec::new();
    for (index, option) in list.iter().enumerate() {
        let name = option["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("/options/{index}/name: must be a non-empty text"))?;
        if option.get("description").is_some_and(|d| !d.is_string()) {
            return Err(format!("/options/{index}/description: must be a text"));
        }
        names.push(name);
    }
    check_unique("/options", &names)
}

fn check_levels(levels: Option<&Value>) -> Result<(), String> {
    let list = levels
        .and_then(Value::as_array)
        .ok_or("/levels: needs a list of level descriptions")?;
    check_count("/levels", list.len())?;
    let mut texts = Vec::new();
    for (index, level) in list.iter().enumerate() {
        let text = level
            .as_str()
            .filter(|text| !text.is_empty())
            .ok_or_else(|| format!("/levels/{index}: must be a non-empty text"))?;
        texts.push(text);
    }
    check_unique("/levels", &texts)
}

fn check_count(pointer: &str, count: usize) -> Result<(), String> {
    if (1..=MAX_ANSWERS).contains(&count) {
        Ok(())
    } else {
        Err(format!(
            "{pointer}: needs 1 to {MAX_ANSWERS} entries, got {count}"
        ))
    }
}

fn check_unique(pointer: &str, names: &[&str]) -> Result<(), String> {
    for (index, name) in names.iter().enumerate() {
        if names[..index].contains(name) {
            return Err(format!("{pointer}/{index}: '{name}' is not unique"));
        }
    }
    Ok(())
}

/// Checks the parameters of `on_decision`: a non-empty list of unique answers.
pub(crate) fn check_on_decision(params: &Value) -> Result<(), String> {
    let list = params["answers"]
        .as_array()
        .filter(|list| !list.is_empty())
        .ok_or("/answers: needs a non-empty list of answers")?;
    let mut answers = Vec::new();
    for (index, answer) in list.iter().enumerate() {
        answers.push(
            answer
                .as_str()
                .ok_or_else(|| format!("/answers/{index}: must be a text"))?,
        );
    }
    check_unique("/answers", &answers)
}

/// All answers a `decide` node with these (checked) parameters can give: option names, `yes`
/// and `no`, or level descriptions; `None` for unusable parameters.
pub fn decision_answers(params: &Value) -> Option<Vec<String>> {
    let texts = |list: &Value, field: Option<&str>| -> Option<Vec<String>> {
        list.as_array()?
            .iter()
            .map(|entry| {
                let text = field.map_or(Some(entry), |field| entry.get(field))?;
                text.as_str().map(str::to_owned)
            })
            .collect()
    };
    match params["kind"].as_str()? {
        "choice" => texts(&params["options"], Some("name")),
        "yes_no" => Some(vec!["yes".to_owned(), "no".to_owned()]),
        "score" => texts(&params["levels"], None),
        _ => None,
    }
}
