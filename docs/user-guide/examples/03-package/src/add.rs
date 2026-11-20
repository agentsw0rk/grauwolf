let a = args["a"].as_f64().ok_or("a is missing")?;
let b = args["b"].as_f64().ok_or("b is missing")?;
Ok((a + b).to_string())
