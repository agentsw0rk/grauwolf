//! Wraps every code body in `src/` in a block, so that the tests can `include!` it as the body of
//! a function – the way grauwolf embeds it.

fn main() {
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=src");
    for entry in std::fs::read_dir("src").unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|extension| extension == "rs") {
            let body = std::fs::read_to_string(&path).unwrap();
            let name = path.file_name().unwrap();
            std::fs::write(out.join(name), format!("{{\n{body}\n}}\n")).unwrap();
        }
    }
}
