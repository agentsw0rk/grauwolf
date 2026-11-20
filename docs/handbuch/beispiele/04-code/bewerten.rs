let lower = input.to_lowercase();
let positive = lower
    .split(|c: char| !c.is_alphabetic())
    .any(|word| ["gut", "toll", "super", "liebe", "hervorragend", "zufrieden", "positiv", "good", "great", "positive"].contains(&word));
if input.trim().is_empty() {
    return Err("the verdict is empty".to_owned());
}
Ok(Verdict {
    text: input.trim().to_owned(),
    positive,
    words: input.split_whitespace().count() as i64,
})
