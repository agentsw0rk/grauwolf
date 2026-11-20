let text = args["text"].as_str().ok_or("text is missing")?;
let count = text.split_whitespace().count();
Ok(format!("{count} words"))
