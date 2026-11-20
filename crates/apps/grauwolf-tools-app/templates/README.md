# {{name}}

Tools for grauwolf agents, built as WASM and published as a tool package. Scaffolded with
`grauwolf-tools new {{name}}`.

```text
grauwolf-tools.json                 name, version, description, license, authors; per tool its
                                    description, argument schema, sandbox limits, capabilities
src/{{tool}}.rs                     the code body of the tool {{tool}}
tests/{{tool}}.rs                   tests of the code body (`cargo test`)
Cargo.toml, tests/support/          only for `cargo test`; grauwolf ignores them
.github/workflows/grauwolf-tools.yml  CI: validate, test, pack, and on a tag `v*` sign and push
```

A tool is the body of `fn tool(args: serde_json::Value) -> Result<String, String>`. It runs in
grauwolf's sandbox: no files, no network (tool ABI v1, so `capabilities` stays empty). `limits`
asks for stricter sandbox limits per tool (`memory_mib`, `fuel`, `time_ms`); the runtime applies
the stricter of these and its own. Add a tool by adding an entry to `tools` and a file
`src/<tool>.rs`. `source` may be set in `grauwolf-tools.json`; without it, packing takes the
`origin` remote of the git checkout, and the commit as the revision.

## Build and check

You need `grauwolf-tools` (`cargo install --git https://github.com/agentsw0rk/grauwolf
grauwolf-tools-app`, or the image `ghcr.io/agentsw0rk/grauwolf-tools`) and the Rust toolchain
with the target `wasm32-unknown-unknown` (`rustup target add wasm32-unknown-unknown`; the image
has both).

```sh
grauwolf-tools validate .     # checks grauwolf-tools.json and src/ without building
cargo test                    # runs tests/
grauwolf-tools pack .         # builds every tool and writes the OCI image layout to dist/
```

## Publish

```sh
grauwolf-tools keygen signing          # once: signing.key (keep it secret) and signing.pub
grauwolf-tools push dist registry.example.com/team/{{name}}:0.1.0 --sign-key signing.key
```

`push` uploads the package, signs it, and adds the version to the tool catalog of the registry
(`registry.example.com/grauwolf-catalog:latest`; `--catalog` names another one, `--no-catalog`
skips it). Credentials come from `GRAUWOLF_REGISTRY_AUTH` or the Docker configuration. Users of
the registry then find the package with

```sh
grauwolf-tools catalog list --catalog registry.example.com
grauwolf-tools catalog show {{name}} --catalog registry.example.com
```

and use it in a spec by its reference, pinned with `grauwolf tools lock`:

```json
"tool_packages": [{ "oci": "registry.example.com/team/{{name}}:0.1.0" }]
```

## CI

`.github/workflows/grauwolf-tools.yml` runs in the image `ghcr.io/agentsw0rk/grauwolf-tools`, so
CI needs no Rust setup. Every push and pull request validates, tests and packs; a tag `v<version>`
(the `version` of `grauwolf-tools.json`) also pushes, signs and adds the catalog entry. Set in the
repository settings:

- secret `GRAUWOLF_REGISTRY_AUTH`: `{"registry.example.com": "user:token"}` with push rights;
- secret `GRAUWOLF_TOOLS_SIGNING_KEY`: the content of `signing.key`;
- variable `TOOLS_REGISTRY` (e.g. `registry.example.com`) and, if needed, `TOOLS_REPOSITORY`
  (default `<owner>/{{name}}`).
