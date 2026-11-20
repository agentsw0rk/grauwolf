//! Serves a folder as a git repository for live checks:
//! `cargo run -p grauwolf-git --features test-git-server --example serve_folder -- <folder> <path-in-repo> <port>`
//! commits the files of `<folder>` below `<path-in-repo>` on branch `main` (tagged `v1`) into a
//! scratch repository and serves it on `<bind>:<port>` (default 127.0.0.1; `0` picks a port).
//! Prints the URL and the commit; stops with Ctrl-C. The repository is writable (push).
//!
//! `--spiffe <mapping-file>` serves over mTLS (ADR-0110): the server's SVID and the bundle come
//! from the Workload API at `SPIFFE_ENDPOINT_SOCKET`, client certificates are required (the own
//! trust domain, more with `--trust-domain`), and the mapping file
//! `{ "<spiffe id or prefix/*>": { "user": "<name>", "push": true|false } }` decides who may read
//! and push (everyone else: 403). It is read again at every request. For tests and live checks
//! only, never for production.

use std::collections::BTreeMap;
use std::path::Path;

use grauwolf_config::ProcessEnv;
use grauwolf_git::testing::{SpiffeAccess, TestGitServer, commit_files, tag};
use grauwolf_spiffe::{SpiffeWorkload, TlsIdentity, WorkloadApiClient};

fn collect(root: &Path, dir: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let relative = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if path.is_dir() {
            collect(root, &path, prefix, out);
        } else {
            let name = if prefix.is_empty() {
                relative
            } else {
                format!("{prefix}/{relative}")
            };
            out.insert(name, std::fs::read(&path).unwrap());
        }
    }
}

const USAGE: &str = "usage: serve_folder <folder> <path-in-repo> <port> \
[--bind <address>] [--spiffe <mapping-file> [--trust-domain <domain>]...]";

#[tokio::main]
async fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut take = |flag: &str| -> Vec<String> {
        let mut values = Vec::new();
        while let Some(index) = args.iter().position(|arg| arg == flag) {
            if index + 1 >= args.len() {
                eprintln!("{flag} needs a value\n{USAGE}");
                std::process::exit(2);
            }
            values.push(args.remove(index + 1));
            args.remove(index);
        }
        values
    };
    let bind = take("--bind")
        .pop()
        .unwrap_or_else(|| "127.0.0.1".to_owned());
    let mapping = take("--spiffe").pop();
    let extra_domains = take("--trust-domain");
    let [folder, prefix, port] = &args[..] else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let mut files = BTreeMap::new();
    let root = Path::new(folder);
    collect(root, root, prefix.trim_matches('/'), &mut files);
    let repository =
        std::env::temp_dir().join(format!("grauwolf-serve-folder-{}", std::process::id()));
    let commit = commit_files(&repository, "main", "package", &files);
    tag(&repository, "v1", commit, true);
    let listener = tokio::net::TcpListener::bind(format!("{bind}:{port}"))
        .await
        .unwrap();
    let server = match mapping {
        None => TestGitServer::serve(listener, &repository, None),
        Some(mapping) => {
            // mTLS mode (ADR-0110): the server's SVID and bundle come from the Workload API at
            // SPIFFE_ENDPOINT_SOCKET; the mapping file decides who may read and push.
            let access = SpiffeAccess::from_file(Path::new(&mapping)).unwrap_or_else(|error| {
                eprintln!("{error}");
                std::process::exit(1);
            });
            let client = WorkloadApiClient::from_env(&ProcessEnv)
                .unwrap_or_else(|error| {
                    eprintln!("{error}");
                    std::process::exit(1);
                })
                .unwrap_or_else(|| {
                    eprintln!("--spiffe needs SPIFFE_ENDPOINT_SOCKET");
                    std::process::exit(1);
                });
            let workload = SpiffeWorkload::connect(client)
                .await
                .unwrap_or_else(|error| {
                    eprintln!("{error}");
                    std::process::exit(1);
                });
            let own = workload.id().expect("the workload has a SPIFFE ID");
            let mut accepted = vec![own.trust_domain().to_owned()];
            accepted.extend(extra_domains);
            println!("spiffe: serving as {own}, mapping {mapping}");
            TestGitServer::serve_spiffe(listener, &repository, &workload, &accepted, access)
                .unwrap_or_else(|error| {
                    eprintln!("{error}");
                    std::process::exit(1);
                })
        }
    };
    println!("url:    {}", server.url("tools"));
    println!("commit: {commit}");
    tokio::signal::ctrl_c().await.unwrap();
    let _ = std::fs::remove_dir_all(&repository);
}
