use std::io::IsTerminal;

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let code = runtime.block_on(async {
        let mut stdout = std::io::stdout();
        let mut stderr = std::io::stderr();
        let mut stdin = std::io::stdin().lock();
        let prompt = |label: &str| rpassword::prompt_password(label);
        let mut io = grauwolf_k8_ops_app::Io::new(&mut stdout, &mut stderr, &mut stdin);
        if std::io::stdin().is_terminal() {
            io = io.with_hidden_prompt(&prompt);
        }
        grauwolf_k8_ops_app::run(std::env::args(), &mut io).await
    });
    std::process::exit(code);
}
